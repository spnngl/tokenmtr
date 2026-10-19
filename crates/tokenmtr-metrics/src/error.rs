//! Errors of the metrics crate.

use tokenmtr_store::StoreError;

/// Why a snapshot could not be computed.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    /// The store refused a read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An SQL statement failed.
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    /// A stored value does not parse into its domain type.
    #[error("stored {what} {value:?} is invalid")]
    Decode {
        /// The column or concept.
        what: &'static str,
        /// The offending value.
        value: String,
    },
    /// Calendar or time-zone arithmetic left the supported range.
    #[error("time: {0}")]
    Time(#[from] jiff::Error),
    /// A remote-write body could not be compressed.
    #[error("snappy: {0}")]
    Snappy(#[from] snap::Error),
}

impl MetricsError {
    pub(crate) fn decode(what: &'static str, value: impl Into<String>) -> Self {
        Self::Decode {
            what,
            value: value.into(),
        }
    }
}
