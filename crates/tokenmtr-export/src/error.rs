//! Errors of the export pipeline.

use std::io;

use camino::Utf8PathBuf;
use tokenmtr_core::IdError;
use tokenmtr_store::StoreError;

/// Why a sink failed to deliver a batch. Recorded in `sink_run.error` and shown by sink health.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    /// The OS refused access to a path: typically a macOS-protected folder.
    #[error(
        "access to {path} was refused; macOS protects Documents, Desktop, Downloads and cloud \
         folders: grant Full Disk Access or Files and Folders to the TokenMtr binary, or point \
         the sink at a folder outside them"
    )]
    PermissionDenied {
        /// The refused path.
        path: Utf8PathBuf,
    },
    /// Any other filesystem failure.
    #[error("{op} {path}: {source}")]
    Io {
        /// What was attempted.
        op: &'static str,
        /// The path involved.
        path: Utf8PathBuf,
        /// The OS error.
        source: io::Error,
    },
    /// An existing export file does not match this build's schema or is damaged.
    #[error("{path} is not a valid export file: {reason}")]
    Corrupt {
        /// The file.
        path: Utf8PathBuf,
        /// What is wrong.
        reason: String,
    },
    /// CSV encoding failed.
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
    /// JSON encoding failed.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// `SQLite` rejected a statement.
    #[error("sqlite {path}: {source}")]
    Sqlite {
        /// The database file.
        path: Utf8PathBuf,
        /// The `SQLite` error.
        source: rusqlite::Error,
    },
    /// The `SQLite` export file was written by another export schema version.
    #[error("{path} holds export schema version {found}, this build writes {expected}")]
    SchemaVersion {
        /// The database file.
        path: Utf8PathBuf,
        /// Version found in `_schema`.
        found: i64,
        /// Version this build writes.
        expected: u32,
    },
}

impl SinkError {
    /// Classifies an I/O error, mapping `PermissionDenied` to its own variant.
    pub(crate) fn io(op: &'static str, path: &camino::Utf8Path, source: io::Error) -> Self {
        if source.kind() == io::ErrorKind::PermissionDenied {
            Self::PermissionDenied {
                path: path.to_owned(),
            }
        } else {
            Self::Io {
                op,
                path: path.to_owned(),
                source,
            }
        }
    }
}

/// Why one export run of a sink stopped.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// Reading batches, cursors or recording runs failed.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// A stored row has no valid export identity.
    #[error("row id: {0}")]
    Id(#[from] IdError),
    /// The sink did not deliver.
    #[error(transparent)]
    Sink(#[from] SinkError),
}
