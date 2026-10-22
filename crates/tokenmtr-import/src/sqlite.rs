//! Reads one `SQLite` export file written by a `[[sinks]] kind = "sqlite"`.
//!
//! The file is copied into a private temp file first and the copy is opened read-only: reading
//! never locks the shared file, never leaves a journal in a synced folder and never sees a file a
//! sync client half wrote. The copy is trusted only when the file's size and modification time
//! are the same before and after it, and no `-journal` sits beside it.

use std::fs;

use camino::Utf8Path;
use jiff::Timestamp;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};
use tempfile::NamedTempFile;
use tokenmtr_core::{Dataset, MachineId};
use tokenmtr_export::schema;
use tokenmtr_store::{ImportFingerprint, Revision};

use crate::decode;
use crate::error::{DecodeError, ImportError, Problem};
use crate::source::{ImportSource, Rows};

/// Export schema versions this build maps to store rows. A version bump in
/// `tokenmtr_export` adds its mapping here (and a decoder test) before it ships.
pub const SUPPORTED_VERSIONS: [u32; 1] = [4];

/// An export file, opened through a private copy.
#[derive(Debug)]
pub struct SqliteExport {
    conn: Connection,
    fingerprint: ImportFingerprint,
    /// Removes the copy on drop.
    _copy: NamedTempFile,
}

/// Size and modification time: what `import_cursor` remembers of a finished read.
fn stat(path: &Utf8Path) -> Result<(u64, Timestamp), ImportError> {
    let meta = fs::metadata(path).map_err(ImportError::io)?;
    let modified = meta.modified().map_err(ImportError::io)?;
    let mtime = Timestamp::try_from(modified)
        .map_err(|_| ImportError::io(std::io::Error::other("modification time out of range")))?;
    // The store keeps microseconds; a finer stamp would never equal the stored one.
    let mtime = Timestamp::from_microsecond(mtime.as_microsecond())
        .map_err(|_| ImportError::io(std::io::Error::other("modification time out of range")))?;
    Ok((meta.len(), mtime))
}

fn journal_exists(path: &Utf8Path) -> bool {
    fs::exists(format!("{path}-journal")).unwrap_or(true)
}

/// The size and modification time of `path`, without opening it.
///
/// # Errors
/// [`ImportError::PermissionDenied`] or [`ImportError::Io`] when the file cannot be stat-ed.
pub fn file_stamp(path: &Utf8Path) -> Result<(u64, Timestamp), ImportError> {
    stat(path)
}

impl SqliteExport {
    /// Copies `path` and opens the copy.
    ///
    /// # Errors
    /// [`ImportError::Busy`] when the file is being written; [`ImportError::NotAnExport`],
    /// [`ImportError::Version`] or [`ImportError::MissingColumn`] when it is not an export this
    /// build reads; [`ImportError::Sqlite`] when `SQLite` refuses the copy.
    pub fn open(path: &Utf8Path) -> Result<Self, ImportError> {
        if journal_exists(path) {
            return Err(ImportError::Busy);
        }
        let before = stat(path)?;
        let copy = NamedTempFile::new().map_err(ImportError::io)?;
        fs::copy(path, copy.path()).map_err(ImportError::io)?;
        if stat(path)? != before || journal_exists(path) {
            return Err(ImportError::Busy);
        }
        let conn = Connection::open_with_flags(
            copy.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "query_only", true)?;
        let version = version(&conn)?;
        for dataset in [Dataset::UsageEvents, Dataset::ToolCalls] {
            check_columns(&conn, dataset)?;
        }
        Ok(Self {
            conn,
            fingerprint: ImportFingerprint {
                size: before.0,
                mtime: before.1,
                schema_version: version,
            },
            _copy: copy,
        })
    }

    /// Whether the file has a table for `dataset`; a sink that exports one dataset has no other.
    fn has_table(&self, dataset: Dataset) -> Result<bool, ImportError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [dataset.as_str()],
            |row| row.get(0),
        )?)
    }

    fn select(dataset: Dataset) -> String {
        let columns: Vec<String> = schema(dataset)
            .columns
            .iter()
            .map(|column| format!("\"{}\"", column.sql_name()))
            .collect();
        format!(
            "SELECT {} FROM \"{dataset}\" WHERE revision > ?1 ORDER BY revision",
            columns.join(", ")
        )
    }
}

fn version(conn: &Connection) -> Result<u32, ImportError> {
    let present: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_schema')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        return Err(ImportError::NotAnExport);
    }
    let found: i64 = conn
        .query_row("SELECT version FROM _schema WHERE id = 1", [], |row| {
            row.get(0)
        })
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => ImportError::NotAnExport,
            other => ImportError::Sqlite(other),
        })?;
    match u32::try_from(found) {
        Ok(version) if SUPPORTED_VERSIONS.contains(&version) => Ok(version),
        _ => Err(ImportError::Version {
            found: u32::try_from(found).unwrap_or(u32::MAX),
            supported: SUPPORTED_VERSIONS.map(|v| v.to_string()).join(", "),
        }),
    }
}

/// A table that exists must have every column of the schema; extra columns are ignored.
fn check_columns(conn: &Connection, dataset: Dataset) -> Result<(), ImportError> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info(\"{dataset}\")"))?;
    let present = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if present.is_empty() {
        return Ok(());
    }
    for column in schema(dataset).columns {
        let name = column.sql_name();
        if !present.iter().any(|p| *p == name) {
            return Err(ImportError::MissingColumn {
                table: dataset.as_str(),
                column: name.into_owned(),
            });
        }
    }
    Ok(())
}

fn decode_error(dataset: Dataset, error: DecodeError) -> ImportError {
    if error.problem == Problem::ForeignMachine {
        ImportError::SeveralMachines
    } else {
        ImportError::Decode {
            table: dataset.as_str(),
            error,
        }
    }
}

impl ImportSource for SqliteExport {
    fn fingerprint(&self) -> ImportFingerprint {
        self.fingerprint
    }

    fn machine(&self) -> Result<Option<MachineId>, ImportError> {
        for dataset in [Dataset::UsageEvents, Dataset::ToolCalls] {
            if !self.has_table(dataset)? {
                continue;
            }
            let found: Option<String> = self
                .conn
                .query_row(
                    &format!("SELECT \"machine_id\" FROM \"{dataset}\" LIMIT 1"),
                    [],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            if let Some(text) = found {
                let machine = text.parse().map_err(|_| {
                    decode_error(
                        dataset,
                        DecodeError {
                            column: "machine_id",
                            problem: Problem::Invalid,
                        },
                    )
                })?;
                return Ok(Some(machine));
            }
        }
        Ok(None)
    }

    fn max_revision(&self, dataset: Dataset) -> Result<Revision, ImportError> {
        if !self.has_table(dataset)? {
            return Ok(Revision::ZERO);
        }
        Ok(self.conn.query_row(
            &format!("SELECT COALESCE(MAX(revision), 0) FROM \"{dataset}\""),
            [],
            |row| row.get(0),
        )?)
    }

    fn read(
        &self,
        dataset: Dataset,
        machine: &MachineId,
        after: Revision,
        page_size: usize,
        on_page: &mut dyn FnMut(Rows) -> Result<bool, ImportError>,
    ) -> Result<bool, ImportError> {
        if !self.has_table(dataset)? {
            return Ok(true);
        }
        let width = schema(dataset).columns.len();
        let mut statement = self.conn.prepare(&Self::select(dataset))?;
        let mut cursor = statement.query([after])?;
        let mut page = Rows::default();
        let mut seen = 0;
        while let Some(row) = cursor.next()? {
            let values = (0..width)
                .map(|i| row.get::<_, Value>(i))
                .collect::<Result<Vec<_>, _>>()?;
            match dataset {
                Dataset::ToolCalls => page.tool_calls.push(
                    decode::tool_call(machine, &values).map_err(|e| decode_error(dataset, e))?,
                ),
                _ => page.events.extend(
                    decode::usage_event(machine, &values).map_err(|e| decode_error(dataset, e))?,
                ),
            }
            seen += 1;
            if seen >= page_size {
                seen = 0;
                if !on_page(std::mem::take(&mut page))? {
                    return Ok(false);
                }
            }
        }
        if seen > 0 && !on_page(page)? {
            return Ok(false);
        }
        Ok(true)
    }
}
