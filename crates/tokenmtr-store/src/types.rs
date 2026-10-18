//! Types shared by every part of the store API.

use std::fmt;

use jiff::Timestamp;
use rusqlite::ToSql;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use tokenmtr_core::{Subscription, UsageEvent};

/// A position in one dataset's change log (D17): the per-dataset counter value a row got when it
/// was inserted or last changed. `Revision::ZERO` precedes every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Revision(u64);

impl Revision {
    /// The position before the first row.
    pub const ZERO: Self = Self(0);

    /// Wraps a raw counter value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw counter value, as exported in the `revision` column.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next counter value.
    #[must_use]
    pub(crate) const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ToSql for Revision {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        i64::try_from(self.0)
            .map(ToSqlOutput::from)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    }
}

impl FromSql for Revision {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_i64()?;
        u64::try_from(raw)
            .map(Self)
            .map_err(|_| FromSqlError::OutOfRange(raw))
    }
}

pub use tokenmtr_core::Dataset;

/// The table holding a dataset's rows.
pub(crate) const fn table(dataset: Dataset) -> &'static str {
    match dataset {
        Dataset::UsageEvents => "usage_event",
        Dataset::ToolCalls => "tool_call",
        Dataset::Subscriptions => "subscription",
        Dataset::QuotaSnapshots => "quota_snapshot",
        Dataset::FxRates => "fx_rate",
    }
}

/// A row of an exported dataset with the revision it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Versioned<T> {
    /// Position in the dataset's change log.
    pub revision: Revision,
    /// The row.
    pub row: T,
}

/// A stored usage event with the state only the store knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    /// The event as ingested.
    pub event: UsageEvent,
    /// A stats-cache aggregate (D38) on a day transcript events now cover. Kept (D14) and
    /// exported with the flag; every query skips it, so the day is not counted twice.
    pub superseded: bool,
}

/// A mirrored subscription with the state only the store knows.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSubscription {
    /// The subscription as last seen in the config.
    pub subscription: Subscription,
    /// When it disappeared from the config; `None` while it is configured. A removed
    /// subscription is kept and exported (D14) but bills nothing.
    pub removed_at: Option<Timestamp>,
}

/// What one batched upsert did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpsertStats {
    /// Rows that did not exist.
    pub inserted: u64,
    /// Existing rows whose values changed; they got a new revision.
    pub updated: u64,
    /// Rows identical to what is stored; they keep their revision.
    pub unchanged: u64,
}

impl UpsertStats {
    /// Rows written: inserted plus updated.
    #[must_use]
    pub const fn changed(&self) -> u64 {
        self.inserted + self.updated
    }
}

/// Rows past a cursor: what a sink still has to export (D41).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    /// How many rows have a revision above the cursor.
    pub rows: u64,
    /// When the oldest of them was written or last changed (not its event time).
    pub oldest: Option<Timestamp>,
}

/// The 32 random bytes that key redaction hashes (D40). They live in `meta`, so\n/// every backup carries them: whoever holds a backup can recompute the hashes in an export..
#[derive(Clone, PartialEq, Eq)]
pub struct RedactionKey([u8; 32]);

impl RedactionKey {
    /// Wraps stored key bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The first 16 hex characters of the blake3 hash of `value` keyed with this key (D40).
    /// Equal inputs give equal hashes, and without the key nobody can test hashes against a list
    /// of known values.
    #[must_use]
    pub fn hash(&self, value: &str) -> String {
        let hex = blake3::keyed_hash(&self.0, value.as_bytes()).to_hex();
        hex.as_str()[..REDACTION_HASH_CHARS].to_owned()
    }
}

/// Hex characters kept from a keyed hash.
const REDACTION_HASH_CHARS: usize = 16;

impl fmt::Debug for RedactionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactionKey(..)")
    }
}

/// Where the next page of a keyset-paged read resumes. Opaque: only the read that returned it
/// can make sense of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCursor(pub(crate) i64);

/// One page of a keyset-paged read.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    /// The rows of this page.
    pub items: Vec<T>,
    /// Where to resume; `None` when this page reached the end.
    pub next: Option<PageCursor>,
}
