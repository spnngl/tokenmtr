//! Errors an agent source can return. Per-line problems are never errors: they become
//! [`ParseIssue`](crate::ParseIssue)s and drift counters (D5, D42).

use camino::Utf8PathBuf;
use tokenmtr_core::IdError;

/// Why an agent source could not read a file at all.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// The file could not be opened or read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// File that failed.
        path: Utf8PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A constant identifier of the agent was rejected by the core types.
    #[error("invalid identifier: {0}")]
    Id(#[from] IdError),
}
