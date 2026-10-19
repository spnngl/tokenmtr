//! `SQLite` store of `TokenMtr`: schema, migrations, the single writer and read-only connections.
//!
//! The write side lives here; `tokenmtr-query` builds the screens on [`StoreReader::connection`]
//! and the schema in `migrations/` (every column of `0001_init.sql` is commented there).
//!
//! # Connections
//!
//! - [`Store`]: the one writer (daemon). WAL, `synchronous=NORMAL`, `fullfsync` and
//!   `checkpoint_fullfsync`. Creates and migrates the file.
//! - [`StoreReader`]: read-only (`SQLITE_OPEN_READ_ONLY`, `query_only`). Never blocks the writer.
//! - [`StoreRead`]: the reads both share (export batches, cursors, checkpoints, backup).
//!
//! # Schema version
//!
//! `PRAGMA user_version` is the schema version; there is no copy in `meta`, so the two cannot
//! disagree. It equals the number of applied migrations ([`SCHEMA_VERSION`]). Both connections
//! refuse a file whose version is higher than this build's with [`StoreError::NewerSchema`] before
//! touching it; [`StoreReader`] also refuses a lower one ([`StoreError::OlderSchema`])
//! because only the writer migrates.
//!
//! # Time and money
//!
//! - Instants are integer UTC epoch **microseconds** (`*_us` columns), the resolution of
//!   [`jiff::Timestamp`], so they round-trip without loss. Civil dates (subscription bounds, FX
//!   dates) are ISO-8601 text.
//! - `usage_event.utc_day` is the only denormalised time column: a stored generated column
//!   holding the UTC day (days since the epoch). It serves the FX lookup ("strictly before the
//!   event's UTC date") and UTC partitioning. Zone-aware day, week and month buckets are
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
//! data changes nothing and costs no revision. Revisions are unique per dataset and gap-free.
//! Rows of exported datasets are never deleted (events are kept forever, and sinks have no delete
//! channel): stats-cache aggregates that transcripts cover are flagged `superseded` instead
//! ([`Store::supersede_daily_aggregates_covered_by_transcripts`]) and subscriptions removed from
//! config get `removed_at`. An exporter reads `revision > cursor ORDER BY revision LIMIT n` through
//! [`StoreRead::usage_events_after`] and its siblings, then moves its cursor with
//! [`Store::set_sink_cursor`]. `updated_at_us` records when the revision was assigned, which is
//! what sink lag is measured from.
//!
//! # Layout
//!
//! - `db`: opening, pragmas, [`Store`], [`StoreReader`], [`StoreRead`].
//! - `versioned`: the generic upsert driver that assigns revisions.
//! - one module per dataset (`events`, `tool_calls`, `subscriptions`, `quota`, `fx`) with its SQL,
//!   writer and reader.
//! - one module per bookkeeping table (`checkpoint`, `signal`, `project`, `alert`, `sink`, `runs`,
//!   `issue`, `pricing`, `account`, `metrics_run`, `meta`).
//! - `ingest`: [`Store::commit_ingest`], the atomic write of one ingest flush.
//! - `backup`: online backup and [`restore`].
//! - `temp`: [`TempStore`], a writer plus readers on a temporary file, for tests.
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
mod ingest;
mod issue;
mod meta;
mod metrics_run;
mod migrations;
mod pricing;
mod project;
mod quota;
mod runs;
mod signal;
mod sink;
mod subscriptions;
mod temp;
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
pub use ingest::{IngestWrite, IngestWriteReport, IssueUpdate, ToolResultFlag};
pub use issue::{IssueKind, OpenParseIssue, ParseIssue};
pub use metrics_run::MetricsRun;
pub use migrations::SCHEMA_VERSION;
pub use pricing::{
    RepriceCursor, RepriceEvent, RepricePage, RepriceScope, Repriced, StoredPricingSnapshot,
};
pub use project::{GitRemote, ProjectResolution, ProjectSource};
pub use quota::{QuotaOutcome, QuotaPoll, QuotaReading, QuotaSample, QuotaStatus};
pub use runs::{BackupRun, IngestRun};
pub use signal::SessionSignal;
pub use sink::SinkRun;
pub use temp::TempStore;
pub use tool_calls::ProfiledToolCall;
pub use types::{
    Dataset, Page, PageCursor, Pending, RedactionKey, Revision, StoredEvent, StoredSubscription,
    UpsertStats, Versioned,
};
