//! The ingest engine (D5, D14–D16, D47, D48): turns agent files into stored events.
//!
//! One scan runs these steps:
//!
//! 1. **Discover** each agent's files, or take the changed paths the caller names.
//! 2. **Compare** every file with its stored checkpoint ([`decide`](fingerprint::decide)): skip,
//!    resume at the offset, or parse from zero after a truncation, replacement or rewrite.
//! 3. **Parse** with the agent's [`AgentSource`](crate::AgentSource). A user-initiated scan
//!    parses files in parallel on a rayon pool; a background scan parses one file at a time.
//! 4. **Write** on the calling thread, the single writer: resolve the project, price, upsert in
//!    transactions of at most 5000 events, then advance the checkpoints of the files whose rows
//!    committed. A crash between the two repeats a parse and writes identical rows.
//! 5. **Import** Claude's stats cache for days before the first transcript event (D38).
//!
//! Nothing here spawns `git` or edits agent files. Every per-file failure becomes a parse issue
//! and the scan goes on.
//!
//! The watcher in [`watch`] debounces raw `notify` events into the path lists a scan takes.

mod doctor;
mod engine;
mod error;
pub mod fingerprint;
mod pricing;
mod projects;
mod report;
mod settings;
pub mod watch;

#[cfg(test)]
mod tests;

pub use doctor::{
    AgentReport, DoctorReport, LastRun, OpenIssue, ProfileReport, Retention, RootReport,
};
pub use engine::{Ingestor, SCAN_ISSUE_SUBJECT, ScanMode, ScanPriority, ScanRequest};
pub use error::IngestError;
pub use fingerprint::ParseReason;
pub use report::{AgentScan, AggregateScan, FileScan, ScanReport, Upserts};
pub use settings::IngestSettings;
