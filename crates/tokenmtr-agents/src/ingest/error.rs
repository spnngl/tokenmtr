//! Errors of the ingest engine. Per-file problems are not errors: they are recorded as issues and
//! the scan goes on (D5). These end a scan.

use camino::Utf8PathBuf;
use tokenmtr_core::{IdError, PricingError};
use tokenmtr_store::StoreError;

/// Why a scan, a doctor run or a re-resolution could not finish.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// The store refused a read or a write.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// A direct query on the read connection failed.
    #[error("store query: {0}")]
    Query(#[from] rusqlite::Error),
    /// The pricing snapshot in the store or in the binary cannot be used.
    #[error("pricing snapshot: {0}")]
    Pricing(#[from] PricingError),
    /// An identifier was rejected by the core types.
    #[error("invalid identifier: {0}")]
    Id(#[from] IdError),
    /// A file or directory could not be read outside a parse (stat, doctor).
    #[error("cannot read {path}: {source}")]
    Io {
        /// What failed.
        path: Utf8PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The worker pool could not start.
    #[error("cannot start the parse pool: {0}")]
    Pool(#[from] rayon::ThreadPoolBuildError),
    /// A stored value is outside what this build understands (a schema drift).
    #[error("unexpected stored value: {0}")]
    Corrupt(String),
    /// A test stopped the writer between the rows and the checkpoints.
    #[cfg(test)]
    #[error("simulated crash before the checkpoints")]
    Crash,
}
