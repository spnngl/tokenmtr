//! Export of `TokenMtr` datasets to sinks: files and `SQLite` (Phase 7).
//!
//! # Pipeline
//!
//! ```text
//! store ──fetch_batch──▶ ExportRow ──Redactor──▶ ExportBatch ──Sink::write──▶ files / SQLite
//!   ▲                                                                   │ durable
//!   └────────────── set_sink_cursor + record_sink_run ◀─────────────────┘
//! ```
//!
//! - [`schema`] names every column once (D19); CSV headers, JSONL keys, `SQLite` DDL
//!   ([`sql`]) and `docs/export-schema.md` all derive from it.
//! - [`project`] maps store rows to [`ExportRow`]s and reads batches.
//! - [`redact`] applies per-sink `hash` and `drop` rules (D40).
//! - [`file`] and [`sqlite`] are the built-in sinks; [`sink`] holds the [`Sink`] trait, the
//!   [`SinkKind`] enum and [`SinkSpec`].
//! - [`pump`] drives sinks from cursors with at-least-once delivery (D17); [`health`] turns the
//!   run history into D41 states.
//!
//! See `docs/export-schema.md` for the consumer contract and `docs/adding-a-sink.md` for adding a
//! sink kind.

pub mod encode;
pub mod error;
pub mod file;
pub mod health;
pub mod project;
pub mod pump;
pub mod redact;
pub mod schema;
pub mod sink;
pub mod sql;
pub mod sqlite;

pub use encode::{Decoded, RowEncoder, StoredRow};
pub use error::{ExportError, SinkError};
pub use file::FileSink;
pub use health::{
    HEALTH_RUNS, HealthInput, LAG_INTERVALS, SinkLag, SinkState, SinkStatus, sink_status,
    total_pending,
};
pub use project::fetch_batch;
pub use pump::{DEFAULT_BATCH_SIZE, PumpOptions, SinkRunOutcome, export_pending};
pub use redact::{Redactor, hash_value};
pub use schema::{
    Column, ColumnType, DatasetSchema, EXPORT_SCHEMA_VERSION, ExportRow, Value, schema,
    schema_markdown,
};
pub use sink::{
    ConfiguredSink, ExportBatch, FileTarget, Sink, SinkKind, SinkSpec, SqliteTarget, Target,
};
pub use sqlite::SqliteSink;
