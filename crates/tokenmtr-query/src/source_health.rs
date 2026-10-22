//! Health of the `[[sources]]`: imports of other machines' exports, and integrations read for
//! hints (Superset).

use std::collections::BTreeSet;

use jiff::Timestamp;
use serde::Serialize;
use tokenmtr_core::SourceId;
use tokenmtr_store::{ImportRun, IssueKind, OpenParseIssue, StoreRead, StoreReader};

use crate::error::QueryError;

/// Runs read per source: a failure streak longer than this reports the oldest run of the window
/// as its start.
pub const SOURCE_HEALTH_RUNS: usize = 200;

/// Runs listed in [`SourceHealth::recent_runs`].
pub const SOURCE_RECENT_RUNS: usize = 10;

/// What a [`SourceSpec`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Superset's database: project and branch hints, no rows.
    Superset,
    /// Exports of other machines: rows.
    Sqlite,
}

/// A configured source, as far as health is concerned (the caller maps its config to this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    /// Source id.
    pub id: SourceId,
    /// What it reads.
    pub kind: SourceKind,
    /// Whether it is switched on.
    pub enabled: bool,
}

/// How a source is doing. Serialises as `{"state": "healthy" | "failing" | "disabled", ...}`. A
/// source that never ran is `healthy`: there is nothing to blame yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SourceState {
    /// The latest pass succeeded, or there was none.
    Healthy,
    /// The latest pass failed (an import), or the data has an unknown shape (Superset).
    Failing {
        /// Start of the first pass of the current streak of failures.
        since: Timestamp,
        /// The latest error.
        last_error: String,
    },
    /// Turned off by the user; never read.
    Disabled,
}

impl SourceState {
    /// Severity for "worst of" aggregation: higher is worse.
    #[must_use]
    pub const fn severity(&self) -> u8 {
        match self {
            Self::Disabled | Self::Healthy => 0,
            Self::Failing { .. } => 1,
        }
    }
}

/// The state of an import after `runs` (any order): failing from the start of the trailing
/// streak of failed passes, when the latest one failed. Only the pass-level runs (no path) count:
/// they carry the outcome of the whole pass.
#[must_use]
pub fn import_state(runs: &[ImportRun]) -> SourceState {
    let mut passes: Vec<&ImportRun> = runs.iter().filter(|run| run.path.is_none()).collect();
    passes.sort_by_key(|run| run.started);
    let Some(latest) = passes.last() else {
        return SourceState::Healthy;
    };
    let Some(last_error) = latest.error.clone() else {
        return SourceState::Healthy;
    };
    let since = passes
        .iter()
        .rev()
        .take_while(|run| run.error.is_some())
        .last()
        .map_or(latest.started, |run| run.started);
    SourceState::Failing { since, last_error }
}

/// One attempt of an import: a pass, or one file of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRunView {
    /// The file; absent for a whole pass.
    pub path: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Timestamp,
    /// Rows written.
    pub rows: u64,
    /// Error, absent on success.
    pub error: Option<String>,
}

impl From<&ImportRun> for SourceRunView {
    fn from(run: &ImportRun) -> Self {
        Self {
            path: run.path.clone(),
            started_at: run.started,
            finished_at: run.finished,
            rows: run.rows,
            error: run.error.clone(),
        }
    }
}

/// Health of one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceHealth {
    /// Source id.
    pub source: SourceId,
    /// What it reads.
    pub kind: SourceKind,
    /// State.
    #[serde(flatten)]
    pub state: SourceState,
    /// Files read so far; zero for a Superset source.
    pub files: u64,
    /// Machines those files came from.
    pub machines: u64,
    /// Usage events held for this source (kept after it leaves the config).
    pub events: u64,
    /// Tool calls held for this source.
    pub tool_calls: u64,
    /// End of the latest successful pass.
    pub last_success_at: Option<Timestamp>,
    /// The latest attempts, newest first (at most [`SOURCE_RECENT_RUNS`]).
    pub recent_runs: Vec<SourceRunView>,
}

/// The state of `spec`: an import by its runs, a Superset source by its open drift issue (named
/// after the source).
fn state_of(spec: &SourceSpec, runs: &[ImportRun], issues: &[OpenParseIssue]) -> SourceState {
    if !spec.enabled {
        return SourceState::Disabled;
    }
    match spec.kind {
        SourceKind::Sqlite => import_state(runs),
        SourceKind::Superset => issues
            .iter()
            .find(|issue| {
                issue.kind == IssueKind::IntegrationDrift && issue.subject == spec.id.as_str()
            })
            .map_or(SourceState::Healthy, |issue| SourceState::Failing {
                since: issue.first_seen,
                last_error: issue
                    .sample
                    .clone()
                    .unwrap_or_else(|| "the database has an unknown shape".to_owned()),
            }),
    }
}

/// Health of every spec.
///
/// # Errors
///
/// Store failures.
pub fn source_health(
    reader: &StoreReader,
    specs: &[SourceSpec],
) -> Result<Vec<SourceHealth>, QueryError> {
    let drifts = reader.open_parse_issues()?;
    specs
        .iter()
        .map(|spec| {
            let runs = reader.import_runs(&spec.id, SOURCE_HEALTH_RUNS)?;
            let cursors = reader.import_cursors(&spec.id)?;
            let rows = reader.imported_rows(&spec.id)?;
            let machines: BTreeSet<_> = cursors.iter().map(|(_, cursor)| cursor.machine).collect();
            let state = state_of(spec, &runs, &drifts);
            Ok(SourceHealth {
                source: spec.id.clone(),
                kind: spec.kind,
                state,
                files: u64::try_from(cursors.len()).unwrap_or(u64::MAX),
                machines: u64::try_from(machines.len()).unwrap_or(u64::MAX),
                events: rows.events,
                tool_calls: rows.tool_calls,
                last_success_at: runs
                    .iter()
                    .find(|run| run.path.is_none() && run.error.is_none())
                    .map(|run| run.finished),
                recent_runs: runs
                    .iter()
                    .take(SOURCE_RECENT_RUNS)
                    .map(SourceRunView::from)
                    .collect(),
            })
        })
        .collect()
}
