//! The one error type of the crate.

use std::io;

use camino::Utf8PathBuf;
use tokenmtr_agents::ingest::IngestError;
use tokenmtr_config::ConfigError;
use tokenmtr_query::{FilterError, QueryError};
use tokenmtr_report::{RenderError, ReportError};
use tokenmtr_store::StoreError;

/// Why an application action failed.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The config is missing, invalid or cannot be edited.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The store cannot be opened, restored or backed up. A newer schema (D52) lands here.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A query failed.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// A date range or filter is invalid.
    #[error(transparent)]
    Filter(#[from] FilterError),
    /// A direct scan or the doctor failed.
    #[error(transparent)]
    Ingest(#[from] IngestError),
    /// The report inputs contradict each other.
    #[error(transparent)]
    Report(#[from] ReportError),
    /// The report cannot be rendered.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// A file operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory.
        path: Utf8PathBuf,
        /// The cause.
        #[source]
        source: io::Error,
    },
    /// The action needs the only writer (D1) gone, but the daemon answers on its socket.
    #[error("the daemon is running (control socket {socket}); stop it first")]
    DaemonRunning {
        /// The socket that accepted a connection.
        socket: Utf8PathBuf,
    },
    /// A request value is out of range or does not name anything.
    #[error("{0}")]
    InvalidRequest(String),
    /// An agent's own settings file cannot be updated (D53).
    #[error("{path}: {reason}")]
    AgentSettings {
        /// The settings file.
        path: Utf8PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// The service manager failed.
    #[error(transparent)]
    Service(#[from] tokenmtr_platform::ServiceError),
}

impl AppError {
    pub(crate) fn io(path: impl Into<Utf8PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
