//! The `parse_issue` table: problems the ingestor met, one open row per subject.

use std::num::NonZeroU64;

use jiff::Timestamp;
use rusqlite::{Connection, named_params};
use serde::{Deserialize, Serialize};
use tokenmtr_core::AgentId;

use crate::codec::{RowExt, enum_text, to_i64, to_us};
use crate::error::StoreError;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// A line shape the parser does not know.
    FormatDrift,
    /// A log format version the parser does not know.
    UnknownVersion,
    /// A folder the OS refuses to let us read.
    ProtectedFolder,
    /// Events without a working directory.
    MissingCwd,
    /// A line that failed to parse.
    ParseError,
    /// Another app's data read for hints (Superset's database) has an unknown shape; the hints
    /// are off. Totals are unaffected.
    IntegrationDrift,
}

impl IssueKind {
    /// The name stored in the database and serialized: snake case.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FormatDrift => "format_drift",
            Self::UnknownVersion => "unknown_version",
            Self::ProtectedFolder => "protected_folder",
            Self::MissingCwd => "missing_cwd",
            Self::ParseError => "parse_error",
            Self::IntegrationDrift => "integration_drift",
        }
    }

    /// Whether the issue only lowers attribution (a project or branch name), never totals:
    /// no event was lost, so the collector has not failed.
    #[must_use]
    pub const fn is_informational(self) -> bool {
        match self {
            Self::ProtectedFolder | Self::MissingCwd | Self::IntegrationDrift => true,
            Self::FormatDrift | Self::UnknownVersion | Self::ParseError => false,
        }
    }
}

/// Something the ingestor noticed, to be counted under `(agent, kind, subject)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIssue {
    /// The agent whose logs were read.
    pub agent: AgentId,
    /// What went wrong.
    pub kind: IssueKind,
    /// What it is about: a file, a field, a version.
    pub subject: String,
    /// How many times it happened in this pass.
    pub count: NonZeroU64,
    /// One example, key names only, never values.
    pub sample: Option<String>,
    /// When it was seen.
    pub at: Timestamp,
}

/// An unresolved issue with its running totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenParseIssue {
    /// The agent whose logs were read.
    pub agent: AgentId,
    /// What went wrong.
    pub kind: IssueKind,
    /// What it is about: a file, a field, a version.
    pub subject: String,
    /// How many times it happened since it opened.
    pub occurrences: u64,
    /// The first example, key names only.
    pub sample: Option<String>,
    /// When it was first seen.
    pub first_seen: Timestamp,
    /// When it was last seen.
    pub last_seen: Timestamp,
}

/// A repeat of an open issue adds to its count and keeps the first sample; a repeat of a resolved
/// issue reopens it from scratch.
const RECORD: &str = "INSERT INTO parse_issue \
    (agent, kind, subject, occurrences, sample, first_seen_us, last_seen_us, resolved_at_us) \
    VALUES (:agent, :kind, :subject, :count, :sample, :at, :at, NULL) \
    ON CONFLICT (agent, kind, subject) DO UPDATE SET \
    occurrences = CASE WHEN resolved_at_us IS NULL \
        THEN occurrences + excluded.occurrences ELSE excluded.occurrences END, \
    sample = CASE WHEN resolved_at_us IS NULL \
        THEN COALESCE(sample, excluded.sample) ELSE excluded.sample END, \
    first_seen_us = CASE WHEN resolved_at_us IS NULL \
        THEN first_seen_us ELSE excluded.first_seen_us END, \
    last_seen_us = excluded.last_seen_us, \
    resolved_at_us = NULL";

/// Counts an occurrence of an issue.
pub(crate) fn record(conn: &Connection, issue: &ParseIssue) -> Result<(), StoreError> {
    conn.prepare_cached(RECORD)?.execute(named_params! {
        ":agent": issue.agent.as_str(),
        ":kind": enum_text(&issue.kind)?,
        ":subject": issue.subject,
        ":count": to_i64(issue.count.get(), "count")?,
        ":sample": issue.sample,
        ":at": to_us(issue.at),
    })?;
    Ok(())
}

/// Every unresolved issue, oldest first.
pub(crate) fn open(conn: &Connection) -> Result<Vec<OpenParseIssue>, StoreError> {
    conn.prepare_cached(
        "SELECT agent, kind, subject, occurrences, sample, first_seen_us, last_seen_us \
         FROM parse_issue WHERE resolved_at_us IS NULL ORDER BY first_seen_us, id",
    )?
    .query_and_then([], |row| {
        Ok(OpenParseIssue {
            agent: row.id::<AgentId>("agent")?,
            kind: row.enumeration("kind")?,
            subject: row.get("subject")?,
            occurrences: row.count("occurrences")?,
            sample: row.get("sample")?,
            first_seen: row.ts("first_seen_us")?,
            last_seen: row.ts("last_seen_us")?,
        })
    })?
    .collect()
}

/// Marks an open issue resolved; `false` when there was none.
pub(crate) fn close(
    conn: &Connection,
    agent: AgentId,
    kind: IssueKind,
    subject: &str,
    at: Timestamp,
) -> Result<bool, StoreError> {
    let closed = conn
        .prepare_cached(
            "UPDATE parse_issue SET resolved_at_us = ?4 \
             WHERE agent = ?1 AND kind = ?2 AND subject = ?3 AND resolved_at_us IS NULL",
        )?
        .execute(rusqlite::params![
            agent.as_str(),
            enum_text(&kind)?,
            subject,
            to_us(at)
        ])?;
    Ok(closed == 1)
}
