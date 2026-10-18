//! The atomic write of one ingest flush.
//!
//! A flush stores events, tool calls, signals, tool-result flags, parse-issue changes and file
//! checkpoints together. Committing the checkpoints in the same transaction as the rows they
//! describe means a crash leaves either both or neither: the file is re-read from the old offset
//! and the idempotent upserts absorb the repeat, with nothing half-applied to reason about.

use jiff::Timestamp;
use rusqlite::Connection;
use tokenmtr_core::{AgentId, UsageEvent};

use crate::checkpoint::{self, Checkpoint};
use crate::error::StoreError;
use crate::events;
use crate::issue::{self, IssueKind, ParseIssue};
use crate::signal::{self, SessionSignal};
use crate::tool_calls::{self, ProfiledToolCall, ResultOutcome};
use crate::types::UpsertStats;

/// The error flag a tool result reports for an earlier tool call, whose line may sit in another
/// file or an earlier flush.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultFlag {
    /// Agent that ran the tool.
    pub agent: AgentId,
    /// Id of the tool use the result answers.
    pub tool_use_id: String,
    /// Whether the tool failed.
    pub is_error: bool,
}

/// A change to the parse-issue table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueUpdate {
    /// Counts an occurrence (opens the issue when it is not open).
    Record(ParseIssue),
    /// Resolves an open issue; nothing happens when there is none.
    Close {
        /// Agent the issue belongs to.
        agent: AgentId,
        /// Issue kind.
        kind: IssueKind,
        /// What the issue is about.
        subject: String,
        /// When it was resolved.
        at: Timestamp,
    },
}

/// Everything one ingest flush stores, applied by [`Store::commit_ingest`](crate::Store).
///
/// Apply order inside the transaction: events, tool calls, signals, tool results, issues,
/// checkpoints. Tool results therefore see the calls of the same flush.
#[derive(Debug, Clone, Copy, Default)]
pub struct IngestWrite<'a> {
    /// Events to upsert; `source_file` ids must come from `Store::save_checkpoint`.
    pub events: &'a [UsageEvent],
    /// Tool calls to upsert.
    pub tool_calls: &'a [ProfiledToolCall],
    /// Session signals to store.
    pub signals: &'a [SessionSignal],
    /// Error flags for stored tool calls.
    pub tool_results: &'a [ToolResultFlag],
    /// Parse-issue changes.
    pub issues: &'a [IssueUpdate],
    /// File checkpoints to save last.
    pub checkpoints: &'a [Checkpoint],
}

/// What [`IngestWrite`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IngestWriteReport {
    /// Outcome of the event upserts.
    pub events: UpsertStats,
    /// Outcome of the tool-call upserts.
    pub tool_calls: UpsertStats,
    /// Signals not stored before.
    pub signals_new: u64,
    /// Tool results that changed a stored call's flag.
    pub tool_results_applied: u64,
    /// Tool results whose call was already flagged the same way.
    pub tool_results_unchanged: u64,
    /// Tool results with no stored call.
    pub tool_results_unmatched: u64,
}

/// Applies `write` on `conn`, which the caller has put inside a write transaction.
pub(crate) fn apply(
    conn: &Connection,
    now: Timestamp,
    write: &IngestWrite<'_>,
) -> Result<IngestWriteReport, StoreError> {
    let mut report = IngestWriteReport {
        events: events::upsert(conn, now, write.events)?,
        tool_calls: tool_calls::upsert(conn, now, write.tool_calls)?,
        signals_new: signal::record(conn, write.signals)?,
        ..IngestWriteReport::default()
    };
    for flag in write.tool_results {
        let outcome =
            tool_calls::apply_result(conn, now, flag.agent, &flag.tool_use_id, flag.is_error)?;
        match outcome {
            ResultOutcome::Applied => report.tool_results_applied += 1,
            ResultOutcome::Unchanged => report.tool_results_unchanged += 1,
            ResultOutcome::Unmatched => report.tool_results_unmatched += 1,
        }
    }
    for update in write.issues {
        match update {
            IssueUpdate::Record(found) => issue::record(conn, found)?,
            IssueUpdate::Close {
                agent,
                kind,
                subject,
                at,
            } => {
                issue::close(conn, *agent, *kind, subject, *at)?;
            }
        }
    }
    for checkpoint in write.checkpoints {
        checkpoint::save(conn, now, checkpoint)?;
    }
    Ok(report)
}
