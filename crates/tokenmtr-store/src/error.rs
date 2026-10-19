//! Store errors.

use tokenmtr_core::billing::BillingError;
use tokenmtr_core::{IdError, MoneyError, PricingError};

/// A stored value that cannot be mapped to its domain type, or the other way round.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// A stored identifier is invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A stored amount or rate is invalid.
    #[error(transparent)]
    Money(#[from] MoneyError),
    /// A stored pricing version is invalid.
    #[error(transparent)]
    Pricing(#[from] PricingError),
    /// A stored subscription breaks a billing rule.
    #[error(transparent)]
    Billing(#[from] BillingError),
    /// A stored timestamp or date is invalid.
    #[error("invalid time: {0}")]
    Time(#[from] jiff::Error),
    /// A stored JSON document or enum name is invalid.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A stored value is outside its domain.
    #[error("column {column} holds an unexpected value {value:?}")]
    Value {
        /// The offending column.
        column: &'static str,
        /// The value, as text.
        value: String,
    },
}

/// Anything the store can fail with.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// `SQLite` failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A schema migration failed.
    #[error("migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    /// A file operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The database was written by a newer `TokenMtr`; it is left untouched.
    #[error("database schema {found} is newer than the supported schema {supported}; upgrade")]
    NewerSchema {
        /// `PRAGMA user_version` of the file.
        found: u32,
        /// [`SCHEMA_VERSION`](crate::SCHEMA_VERSION) of this build.
        supported: u32,
    },
    /// A reader met a database that no writer has migrated yet.
    #[error(
        "database schema {found} is older than {expected}; a daemon of this build migrates it when \
         it starts: stop any older daemon still running, then start this build's \
         (`tokenmtr service install` or `tokenmtr run`)"
    )]
    OlderSchema {
        /// `PRAGMA user_version` of the file.
        found: u32,
        /// [`SCHEMA_VERSION`](crate::SCHEMA_VERSION) of this build.
        expected: u32,
    },
    /// `SQLite` refused write-ahead logging, which the store requires; the file system may
    /// not support it.
    #[error("write-ahead logging is unavailable (journal mode is {0:?})")]
    WalUnavailable(String),
    /// `PRAGMA integrity_check` reported a problem.
    #[error("integrity check failed: {0}")]
    IntegrityCheck(String),
    /// The database lacks a row the schema seeds; it is not a `TokenMtr` store.
    #[error("meta key {0:?} is missing")]
    MissingMeta(&'static str),
    /// A number does not fit `SQLite`'s signed 64-bit integers.
    #[error("{column} does not fit a signed 64-bit integer")]
    OutOfRange {
        /// The offending column.
        column: &'static str,
    },
    /// A stored value cannot be decoded.
    #[error("corrupt data: {0}")]
    Codec(#[from] CodecError),
}

impl From<IdError> for StoreError {
    fn from(error: IdError) -> Self {
        Self::Codec(error.into())
    }
}

impl From<MoneyError> for StoreError {
    fn from(error: MoneyError) -> Self {
        Self::Codec(error.into())
    }
}

impl From<jiff::Error> for StoreError {
    fn from(error: jiff::Error) -> Self {
        Self::Codec(error.into())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Codec(error.into())
    }
}
