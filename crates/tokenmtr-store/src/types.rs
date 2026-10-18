//! Types shared by every part of the store API.

use std::fmt;

use jiff::Timestamp;
use rusqlite::ToSql;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};

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

/// One exportable dataset (Phase 7). Each has its own revision counter and sink cursors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dataset {
    /// `usage_events`.
    UsageEvents,
    /// `tool_calls`.
    ToolCalls,
    /// `subscriptions`.
    Subscriptions,
    /// `quota_snapshots`.
    QuotaSnapshots,
    /// `fx_rates`.
    FxRates,
}

impl Dataset {
    /// Every dataset, in export order.
    pub const ALL: [Self; 5] = [
        Self::UsageEvents,
        Self::ToolCalls,
        Self::Subscriptions,
        Self::QuotaSnapshots,
        Self::FxRates,
    ];

    /// The dataset's export name; also its key in `dataset_revision` and `sink_cursor`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsageEvents => "usage_events",
            Self::ToolCalls => "tool_calls",
            Self::Subscriptions => "subscriptions",
            Self::QuotaSnapshots => "quota_snapshots",
            Self::FxRates => "fx_rates",
        }
    }

    /// Parses an export name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }

    /// The table holding the rows.
    pub(crate) const fn table(self) -> &'static str {
        match self {
            Self::UsageEvents => "usage_event",
            Self::ToolCalls => "tool_call",
            Self::Subscriptions => "subscription",
            Self::QuotaSnapshots => "quota_snapshot",
            Self::FxRates => "fx_rate",
        }
    }
}

impl fmt::Display for Dataset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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

/// The 32 random bytes that key redaction hashes (D33).
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
}

impl fmt::Debug for RedactionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactionKey(..)")
    }
}
