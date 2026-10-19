//! Daemon errors.

use camino::Utf8PathBuf;
use tokenmtr_agents::ingest::IngestError;
use tokenmtr_config::ConfigError;
use tokenmtr_platform::ControlError;
use tokenmtr_query::QueryError;
use tokenmtr_remote::{FxError, RemoteError};
use tokenmtr_store::StoreError;

/// Why the daemon cannot start, or why one of its operations failed.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// The configuration file cannot be read or is invalid.
    #[error("configuration: {0}")]
    Config(#[from] ConfigError),
    /// The store was written by a newer `TokenMtr`; it is left untouched.
    #[error(
        "the store has schema {found} but this tokenmtr supports up to {supported}: upgrade tokenmtr"
    )]
    NewerSchema {
        /// The schema of the store file.
        found: u32,
        /// The newest schema this build reads.
        supported: u32,
    },
    /// The store failed.
    #[error("store: {0}")]
    Store(StoreError),
    /// A scan failed.
    #[error("ingest: {0}")]
    Ingest(IngestError),
    /// A query failed.
    #[error("query: {0}")]
    Query(#[from] QueryError),
    /// The metrics snapshot failed.
    #[error("metrics: {0}")]
    Metrics(#[from] tokenmtr_metrics::MetricsError),
    /// Another daemon owns the control socket.
    #[error("another daemon is already running (control socket {0})")]
    AlreadyRunning(Utf8PathBuf),
    /// Another process (an erase, a restore, a direct scan) holds the daemon lock.
    #[error("another tokenmtr process holds the daemon lock {0}")]
    Locked(Utf8PathBuf),
    /// The control socket cannot be created.
    #[error("control socket: {0}")]
    Control(ControlError),
    /// A network client cannot be built.
    #[error("network: {0}")]
    Remote(#[from] RemoteError),
    /// An FX download failed.
    #[error("exchange rates: {0}")]
    Fx(#[from] FxError),
    /// The file watcher cannot be created.
    #[error("file watcher: {0}")]
    Watch(#[from] notify::Error),
    /// A blocking task panicked or was cancelled.
    #[error("background task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    /// A job could not run because the feature it needs is switched off in the config.
    #[error("{0} is disabled in the configuration")]
    Disabled(&'static str),
    /// A request named something that does not exist.
    #[error("{0}")]
    Unknown(String),
    /// The shutdown deadline passed while a job was running, or shutdown stopped a scan.
    #[error("the daemon is shutting down")]
    ShuttingDown,
}

impl From<IngestError> for DaemonError {
    fn from(error: IngestError) -> Self {
        match error {
            IngestError::Stopped => Self::ShuttingDown,
            other => Self::Ingest(other),
        }
    }
}

impl From<StoreError> for DaemonError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NewerSchema { found, supported } => Self::NewerSchema { found, supported },
            other => Self::Store(other),
        }
    }
}

impl From<ControlError> for DaemonError {
    fn from(error: ControlError) -> Self {
        match error {
            ControlError::AlreadyRunning(path) => Self::AlreadyRunning(path),
            ControlError::Locked(path) => Self::Locked(path),
            other @ ControlError::Io(_) => Self::Control(other),
        }
    }
}
