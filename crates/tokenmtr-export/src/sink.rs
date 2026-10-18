//! The sink abstraction: what a sink receives, how one is configured and the enum
//! that dispatches to the built-in kinds.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::SignedDuration;
use tokenmtr_config::{
    FileFormat, FileMode, Partition, RedactColumn, RedactMode, SinkConfig, SinkTarget,
};
use tokenmtr_core::SinkId;
use tokenmtr_store::{Dataset, Revision};

use crate::error::SinkError;
use crate::file::FileSink;
use crate::schema::ExportRow;
use crate::sqlite::SqliteSink;

/// File name of the default `SQLite` export inside the exports directory.
pub const DEFAULT_SQLITE_FILE: &str = "export.db";

/// Rows of one dataset in ascending revision order, redacted and ready to deliver.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportBatch {
    /// The dataset every row belongs to.
    pub dataset: Dataset,
    /// Rows in ascending revision order.
    pub rows: Vec<ExportRow>,
}

impl ExportBatch {
    /// The highest revision in the batch; the cursor moves here once the batch is durable.
    #[must_use]
    pub fn last_revision(&self) -> Option<Revision> {
        self.rows.last().map(|r| r.revision)
    }
}

/// Something that durably stores batches.
///
/// `write` is idempotent: delivering rows a sink already holds, at the same or a lower revision,
/// changes nothing. It returns only after the rows are durable, because the caller advances the
/// cursor on `Ok`.
pub trait Sink {
    /// The sink's id from config.
    fn id(&self) -> &SinkId;

    /// Stores the batch durably.
    ///
    /// # Errors
    /// [`SinkError`] when any row could not be stored; the cursor then stays and the batch is
    /// delivered again.
    fn write(&mut self, batch: &ExportBatch) -> Result<(), SinkError>;
}

/// Where a file sink writes and how (config's `FileSink` with the directory resolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTarget {
    /// CSV or JSONL.
    pub format: FileFormat,
    /// Output directory.
    pub dir: Utf8PathBuf,
    /// Append or snapshot.
    pub mode: FileMode,
    /// Partitioning by row date.
    pub partition: Partition,
}

/// Where a `SQLite` sink writes (config's `SqliteSink` with the path resolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteTarget {
    /// Database file.
    pub path: Utf8PathBuf,
}

/// The resolved kind-specific part of a sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// CSV or JSONL files.
    File(FileTarget),
    /// A `SQLite` database.
    Sqlite(SqliteTarget),
}

/// A configured sink with every default resolved: the input of [`SinkKind::new`] and the pump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkSpec {
    /// Unique id.
    pub id: SinkId,
    /// Minimum time between runs; health measures lag against it.
    pub interval: SignedDuration,
    /// Datasets this sink receives, in delivery order.
    pub datasets: Vec<Dataset>,
    /// Per-column redaction.
    pub redact: BTreeMap<RedactColumn, RedactMode>,
    /// Kind-specific settings.
    pub target: Target,
}

impl SinkSpec {
    /// Resolves a config entry: unset directories and paths become `exports_dir`.
    #[must_use]
    pub fn from_config(config: &SinkConfig, exports_dir: &Utf8Path) -> Self {
        let target = match &config.target {
            SinkTarget::File(file) => Target::File(FileTarget {
                format: file.format,
                dir: file.dir.clone().unwrap_or_else(|| exports_dir.to_owned()),
                mode: file.mode,
                partition: file.partition,
            }),
            SinkTarget::Sqlite(sqlite) => Target::Sqlite(SqliteTarget {
                path: sqlite
                    .path
                    .clone()
                    .unwrap_or_else(|| exports_dir.join(DEFAULT_SQLITE_FILE)),
            }),
        };
        Self {
            id: config.id.clone(),
            interval: config.interval,
            datasets: config.datasets.clone(),
            redact: config.redact.clone(),
            target,
        }
    }
}

/// The built-in sinks. A Postgres sink would add a variant.
#[derive(Debug)]
pub enum SinkKind {
    /// CSV or JSONL files.
    File(FileSink),
    /// A `SQLite` database.
    Sqlite(SqliteSink),
}

impl SinkKind {
    /// Builds the sink for a spec. Touches no disk: directories, files and connections are created
    /// on the first write, so a refused path surfaces as a failed run.
    #[must_use]
    pub fn new(spec: &SinkSpec) -> Self {
        match &spec.target {
            Target::File(target) => Self::File(FileSink::new(spec.id.clone(), target.clone())),
            Target::Sqlite(target) => {
                Self::Sqlite(SqliteSink::new(spec.id.clone(), target.clone()))
            }
        }
    }
}

impl Sink for SinkKind {
    fn id(&self) -> &SinkId {
        match self {
            Self::File(sink) => sink.id(),
            Self::Sqlite(sink) => sink.id(),
        }
    }

    fn write(&mut self, batch: &ExportBatch) -> Result<(), SinkError> {
        match self {
            Self::File(sink) => sink.write(batch),
            Self::Sqlite(sink) => sink.write(batch),
        }
    }
}

/// A sink with its spec: what [`crate::export_pending`] iterates.
#[derive(Debug)]
pub struct ConfiguredSink {
    /// The resolved configuration.
    pub spec: SinkSpec,
    /// The sink built from it; keeps per-file caches across runs.
    pub kind: SinkKind,
}

impl ConfiguredSink {
    /// Builds the sink of `spec`.
    #[must_use]
    pub fn new(spec: SinkSpec) -> Self {
        let kind = SinkKind::new(&spec);
        Self { spec, kind }
    }
}
