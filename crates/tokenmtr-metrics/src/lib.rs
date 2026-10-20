//! Prometheus metrics computed from the `TokenMtr` store.
//!
//! This crate is pure: it reads a [`tokenmtr_store::StoreReader`] and returns series. The
//! encoders (text, `OpenMetrics`, remote write) and the network outputs build on it.
//!
//! # Model
//!
//! - **A view of the store, not a sink (P1).** Every output reads one [`Snapshot`], recomputed
//!   from the store; there is no cursor and no delta stream. Revisions, re-resolved projects and
//!   daemon restarts cannot count anything twice.
//! - **Absolute counters (P2).** A total family counts the store's whole history.
//! - **No money (P3).** Tokens, event counts, tool-call counts and quota utilization only.
//! - **Rows counted (P4).** `superseded = 0` and `kind NOT IN ('daily_aggregate',
//!   'extra_usage')`.
//! - **Labels (P5).** `agent`, `profile`, `provider`, `model`, `client`, `account`, `billing`,
//!   `project` (the complete normalized git remote, else the directory name; never the key,
//!   which can be a path), `service_tier` and `sidechain` on every usage family. A session
//!   family swaps `agent`, `profile` and `account` for `session_id` and `git_branch`;
//!   `tokenmtr_session_info` links a `session_id` back to them and to the `machine_id`, which
//!   `tokenmtr_machine_info` describes. `cwd` is never a label.
//! - **`billing` is the query engine's rule (P6).** [`tokenmtr_core::covering`], evaluated once
//!   per subscription segment.
//! - **Per-output exclusion and redaction (P7, P20).** [`Snapshot::fold`] applies an
//!   [`OutputScope`]: excluded groups are dropped, labels are hashed or dropped, equal series are
//!   summed. A `NULL` cwd or project is never excluded.
//! - **Session families cover active sessions (P15).** Sessions with a counted event inside
//!   `[metrics].session_window`; [`Snapshot::next_expiry`] says when one leaves.
//! - **`created` (P21).** The earliest row behind every counter series.

pub mod catalog;
mod error;
mod remote_write;
mod scope;
mod snapshot;
#[cfg(test)]
mod tests;
pub mod text;

pub use catalog::{Family, Kind};
pub use error::MetricsError;
pub use remote_write::{MAX_SERIES_PER_REQUEST, Request, Seen, WriteRequest, requests};
pub use scope::{Folded, OutputScope, Series, Value};
pub use snapshot::{Machine, Snapshot, SnapshotContext, snapshot};
