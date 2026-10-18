//! `SQLite` store of `TokenMtr`: schema, migrations, the single writer and read-only connections.
//!
//! The write side lives here; `tokenmtr-query` builds the screens on [`StoreReader::connection`]
//! and the schema in `migrations/0001_init.sql` (every column is commented there).
//!
//! # Connections
//!
//! - [`Store`]: the one writer (daemon). WAL, `synchronous=NORMAL`, `fullfsync` and
//!   `checkpoint_fullfsync` (D6, D49). Creates and migrates the file.
//! - [`StoreReader`]: read-only (`SQLITE_OPEN_READ_ONLY`, `query_only`). Never blocks the writer.
//! - [`StoreRead`]: the reads both share (export batches, cursors, checkpoints, backup).
//!
//! # Schema version
//!
//! `PRAGMA user_version` is the schema version; there is no copy in `meta`, so the two cannot
//! disagree. It equals the number of applied migrations ([`SCHEMA_VERSION`]). Both connections
//! refuse a file whose version is higher than this build's with [`StoreError::NewerSchema`] before
//! touching it (D52); [`StoreReader`] also refuses a lower one ([`StoreError::OlderSchema`])
//! because only the writer migrates.
//!
//! # Time and money
//!
//! - Instants are integer UTC epoch **microseconds** (`*_us` columns), the resolution of
//!   [`jiff::Timestamp`], so they round-trip without loss. Civil dates (subscription bounds, FX
//!   dates) are ISO-8601 text.
//! - `usage_event.utc_day` is the only denormalised time column: a stored generated column
//!   holding the UTC day (days since the epoch). It serves the FX lookup (D27, "strictly before the
//!   event's UTC date") and UTC partitioning. Zone-aware day, week and month buckets (D8) are
//!   computed at query time from `at_us` with the configured IANA zone. They are deliberately not
//!   stored: a stored bucket would go stale when the user changes the zone.
//! - Money is integer USD micros in `*_usd_micros` columns ([`tokenmtr_core::UsdMicros`]);
//!   subscription fees keep their own currency in `fee_micros` + `fee_currency`.
//! - Event ids are 16-byte BLOBs ([`tokenmtr_core::EventId`]).
//!
//! # Revisions
//!
//! Each exported dataset ([`Dataset`]) has one counter in `dataset_revision`. An upsert gives a
//! row the next revision only when a value column differs from the stored row; rewriting identical
//! data changes nothing and costs no revision. Revisions are gap-free and unique per dataset, so
//! an exporter reads `revision > cursor ORDER BY revision LIMIT n` (D17) through
//! [`StoreRead::usage_events_after`] and its siblings, then moves its cursor with
//! [`Store::set_sink_cursor`]. `updated_at_us` records when the revision was assigned, which is
//! what sink lag is measured from (D41).
//!
//! # Layout
//!
//! - `db`: opening, pragmas, [`Store`], [`StoreReader`], [`StoreRead`].
//! - `versioned`: the generic upsert driver that assigns revisions.
//! - one module per dataset (`events`, `tool_calls`, `subscriptions`, `quota`, `fx`) with its SQL,
//!   writer and reader.
//! - one module per bookkeeping table (`checkpoint`, `signal`, `project`, `alert`, `sink`, `runs`,
//!   `issue`, `pricing`, `account`, `meta`).
//! - `backup`: online backup and [`restore`].
//! - `codec`, `types`, `error`, `migrations`: shared plumbing.

mod account;
mod alert;
mod backup;
mod checkpoint;
mod codec;
mod db;
mod error;
mod events;
mod fx;
mod issue;
mod meta;
mod migrations;
mod pricing;
mod project;
mod quota;
mod runs;
mod signal;
mod sink;
mod subscriptions;
mod tool_calls;
mod types;
mod versioned;

#[cfg(test)]
mod tests;

pub use account::AccountRecord;
pub use backup::{BackupInfo, restore};
pub use checkpoint::{Checkpoint, StoredCheckpoint};
pub use db::{Store, StoreRead, StoreReader};
pub use error::{CodecError, StoreError};
pub use fx::{FxRateRow, FxSource};
pub use issue::{IssueKind, ParseIssue};
pub use migrations::SCHEMA_VERSION;
pub use pricing::StoredPricingSnapshot;
pub use project::{ProjectResolution, ProjectSource};
pub use quota::{QuotaOutcome, QuotaPoll, QuotaReading, QuotaSample, QuotaStatus};
pub use runs::{BackupRun, IngestRun};
pub use signal::SessionSignal;
pub use sink::SinkRun;
pub use tool_calls::ProfiledToolCall;
pub use types::{Dataset, Pending, RedactionKey, Revision, UpsertStats, Versioned};
