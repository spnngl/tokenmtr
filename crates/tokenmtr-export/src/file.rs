//! CSV and JSONL file sink (D18, D49).
//!
//! Files are named `<dataset>-<partition>.<csv|jsonl>`, e.g. `usage_events-2026-05.csv`, where
//! the partition comes from the row's UTC date (`YYYY-MM`, `YYYY-MM-DD` or `all`).
//!
//! # Modes
//!
//! - **Append** adds every changed row at the end of its partition file, carrying its revision;
//!   readers keep the highest revision per id. Delivery is idempotent: the sink reads each file's
//!   highest revision once (and again whenever the file's length differs from what it wrote) and
//!   skips rows at or below it. Rows reach a partition in ascending revision order, so "highest
//!   revision present" means "everything up to it was delivered". A torn last line from a crash is
//!   cut off before appending. Each batch is `fsync`ed before `write` returns.
//! - **Snapshot** merges the batch into the partition's existing rows (highest revision per id
//!   wins) and replaces the file atomically: temp file, `fsync`, rename, directory `fsync`. The file
//!   always holds one row per id, ordered by id. A partition is held in memory while it is
//!   rewritten; prefer `partition = "day"` for large datasets.
//!
//! Datasets without a stable date (subscriptions) always use the single `all` partition.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};

use camino::{Utf8Path, Utf8PathBuf};
use jiff::civil::Date;
use tokenmtr_config::{FileFormat, FileMode, Partition};
use tokenmtr_core::SinkId;
use tokenmtr_platform::fs::replace_atomically;
use tokenmtr_store::{Dataset, Revision};

use crate::encode::{RowEncoder, StoredRow};
use crate::error::SinkError;
use crate::schema::{ExportRow, schema};
use crate::sink::{ExportBatch, FileTarget, Sink};

/// What the sink knows about one append file, to avoid rescanning it on every batch.
#[derive(Debug, Clone, Copy)]
struct FileState {
    /// Highest revision in the file.
    max_revision: Revision,
    /// File length after the last whole line; a different on-disk length forces a rescan.
    len: u64,
    /// Whether the header (CSV) has been written.
    has_header: bool,
}

/// Writes datasets as CSV or JSONL files.
#[derive(Debug)]
pub struct FileSink {
    id: SinkId,
    target: FileTarget,
    known: HashMap<Utf8PathBuf, FileState>,
}

impl FileSink {
    /// A sink for `target`. Creates nothing until the first write.
    #[must_use]
    pub fn new(id: SinkId, target: FileTarget) -> Self {
        Self {
            id,
            target,
            known: HashMap::new(),
        }
    }

    /// The path of the partition file that holds rows of `dataset` dated `date`.
    #[must_use]
    pub fn partition_path(&self, dataset: Dataset, date: Date) -> Utf8PathBuf {
        let partition = match (schema(dataset).partition_column, self.target.partition) {
            (None, _) | (Some(_), Partition::All) => "all".to_owned(),
            (Some(_), Partition::Month) => format!("{:04}-{:02}", date.year(), date.month()),
            (Some(_), Partition::Day) => date.to_string(),
        };
        let extension = match self.target.format {
            FileFormat::Csv => "csv",
            FileFormat::Jsonl => "jsonl",
        };
        self.target
            .dir
            .join(format!("{dataset}-{partition}.{extension}"))
    }

    /// Streams the file once (constant memory) for its highest revision, cutting a torn tail.
    fn scan(path: &Utf8Path, encoder: &RowEncoder) -> Result<FileState, SinkError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(FileState {
                    max_revision: Revision::ZERO,
                    len: 0,
                    has_header: false,
                });
            }
            Err(e) => return Err(SinkError::io("read", path, e)),
        };
        let on_disk = file
            .metadata()
            .map_err(|e| SinkError::io("read", path, e))?
            .len();
        let scanned = encoder.scan(path, file)?;
        if scanned.valid_len < on_disk {
            truncate(path, scanned.valid_len)?;
        }
        Ok(FileState {
            max_revision: Revision::new(scanned.max_revision),
            len: scanned.valid_len,
            has_header: scanned.valid_len > 0,
        })
    }

    fn append(
        &mut self,
        path: &Utf8Path,
        encoder: &RowEncoder,
        rows: &[&ExportRow],
    ) -> Result<(), SinkError> {
        let on_disk = fs::metadata(path).map(|m| m.len()).ok();
        let cached =
            self.known.get(path).copied().filter(|state| {
                on_disk == Some(state.len) || (on_disk.is_none() && state.len == 0)
            });
        let state = match cached {
            Some(state) => state,
            None => Self::scan(path, encoder)?,
        };
        let fresh: Vec<&&ExportRow> = rows
            .iter()
            .filter(|r| r.revision > state.max_revision)
            .collect();
        let Some(last) = fresh.last() else {
            self.known.insert(path.to_owned(), state);
            return Ok(());
        };
        let mut buffer = if state.has_header {
            Vec::new()
        } else {
            encoder.header()?
        };
        for row in &fresh {
            buffer.extend(encoder.encode(row)?);
        }
        let last_revision = last.revision;
        append_durably(path, &buffer, !state.has_header)?;
        self.known.insert(
            path.to_owned(),
            FileState {
                max_revision: last_revision,
                len: state.len + to_len(buffer.len()),
                has_header: true,
            },
        );
        Ok(())
    }

    fn snapshot(
        path: &Utf8Path,
        encoder: &RowEncoder,
        rows: &[&ExportRow],
    ) -> Result<(), SinkError> {
        let (existing, torn) = match fs::read(path) {
            Ok(bytes) => {
                let decoded = encoder.decode(path, &bytes)?;
                (decoded.rows, decoded.valid_len < bytes.len())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), false),
            Err(e) => return Err(SinkError::io("read", path, e)),
        };
        let was_empty = existing.is_empty();
        let mut merged: BTreeMap<String, StoredRow> = BTreeMap::new();
        for row in existing {
            keep_highest(&mut merged, row);
        }
        let mut changed = torn || was_empty;
        for row in rows {
            let stored = StoredRow {
                id: row.id(),
                revision: row.revision.get(),
                bytes: encoder.encode(row)?,
            };
            changed |= keep_highest(&mut merged, stored);
        }
        if !changed {
            return Ok(());
        }
        let mut bytes = encoder.header()?;
        for row in merged.values() {
            bytes.extend_from_slice(&row.bytes);
        }
        replace_atomically(path, &bytes).map_err(|e| SinkError::io("replace", path, e))
    }
}

/// Inserts `row` unless a row with the same id and an equal or higher revision exists. Returns
/// whether the map changed.
fn keep_highest(map: &mut BTreeMap<String, StoredRow>, row: StoredRow) -> bool {
    match map.get(&row.id) {
        Some(current) if current.revision >= row.revision => false,
        _ => {
            map.insert(row.id.clone(), row);
            true
        }
    }
}

fn to_len(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

fn truncate(path: &Utf8Path, len: u64) -> Result<(), SinkError> {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| SinkError::io("open", path, e))?;
    file.set_len(len)
        .and_then(|()| file.sync_all())
        .map_err(|e| SinkError::io("truncate", path, e))
}

/// Appends `bytes` and `fsync`s; a new file also `fsync`s its directory so it survives a crash.
fn append_durably(path: &Utf8Path, bytes: &[u8], created: bool) -> Result<(), SinkError> {
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .map_err(|e| SinkError::io("open", path, e))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| SinkError::io("append", path, e))?;
    if created && let Some(dir) = path.parent() {
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| SinkError::io("sync", dir, e))?;
    }
    Ok(())
}

impl Sink for FileSink {
    fn id(&self) -> &SinkId {
        &self.id
    }

    fn write(&mut self, batch: &ExportBatch) -> Result<(), SinkError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.target.dir)
            .map_err(|e| SinkError::io("create", &self.target.dir, e))?;
        let encoder = RowEncoder::new(self.target.format, batch.dataset);
        let mut by_file: BTreeMap<Utf8PathBuf, Vec<&ExportRow>> = BTreeMap::new();
        for row in &batch.rows {
            by_file
                .entry(self.partition_path(batch.dataset, row.partition_date))
                .or_default()
                .push(row);
        }
        for (path, rows) in by_file {
            let result = match self.target.mode {
                FileMode::Append => self.append(&path, &encoder, &rows),
                FileMode::Snapshot => Self::snapshot(&path, &encoder, &rows),
            };
            if let Err(error) = result {
                self.known.remove(&path);
                return Err(error);
            }
        }
        Ok(())
    }
}
