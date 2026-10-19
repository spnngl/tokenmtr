//! Health of the metrics outputs (exporters and remote writes), from their run history.

use jiff::Timestamp;
use serde::Serialize;
use tokenmtr_core::SinkId;
use tokenmtr_store::{MetricsRun, StoreRead, StoreReader};

use crate::error::QueryError;

/// Runs read per output: a failure streak longer than this reports the oldest run of the window
/// as its start.
pub const METRICS_HEALTH_RUNS: usize = 200;

/// Runs listed in [`MetricsHealth::recent_runs`].
pub const METRICS_RECENT_RUNS: usize = 10;

/// What kind of output a [`MetricsSpec`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricsKind {
    /// A scrape endpoint (`[[metrics.exporters]]`).
    Exporter,
    /// A push to a receiver (`[[metrics.remote_write]]`).
    RemoteWrite,
}

/// A configured metrics output, as far as health is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSpec {
    /// Output id.
    pub id: SinkId,
    /// Exporter or remote write.
    pub kind: MetricsKind,
}

/// How an output is doing. Serialises as `{"state": "healthy" | "failing", ...}`. An output
/// that never ran is `healthy`: there is nothing to blame yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MetricsState {
    /// The latest attempt succeeded, or there was none.
    Healthy,
    /// The latest attempt failed.
    Failing {
        /// Start of the first attempt of the current streak of failures.
        since: Timestamp,
        /// Error of the latest failed attempt.
        last_error: String,
    },
}

impl MetricsState {
    /// Severity for "worst of" aggregation: higher is worse.
    #[must_use]
    pub const fn severity(&self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Failing { .. } => 1,
        }
    }
}

/// The state after `runs` (any order): failing from the start of the trailing streak of failed
/// attempts, when the latest one failed.
#[must_use]
pub fn metrics_state(runs: &[MetricsRun]) -> MetricsState {
    let mut sorted: Vec<&MetricsRun> = runs.iter().collect();
    sorted.sort_by_key(|run| run.started);
    let Some(latest) = sorted.last() else {
        return MetricsState::Healthy;
    };
    let Some(last_error) = latest.error.clone() else {
        return MetricsState::Healthy;
    };
    let since = sorted
        .iter()
        .rev()
        .take_while(|run| run.error.is_some())
        .last()
        .map_or(latest.started, |run| run.started);
    MetricsState::Failing { since, last_error }
}

/// One attempt of an output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetricsRunView {
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Timestamp,
    /// Series pushed; zero for a bind and for a failure.
    pub series: u64,
    /// Error, absent on success.
    pub error: Option<String>,
}

impl From<&MetricsRun> for MetricsRunView {
    fn from(run: &MetricsRun) -> Self {
        Self {
            started_at: run.started,
            finished_at: run.finished,
            series: run.series,
            error: run.error.clone(),
        }
    }
}

/// Health of one metrics output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetricsHealth {
    /// Output id.
    pub output: SinkId,
    /// Exporter or remote write.
    pub kind: MetricsKind,
    /// State.
    #[serde(flatten)]
    pub state: MetricsState,
    /// End of the latest successful attempt.
    pub last_success_at: Option<Timestamp>,
    /// The latest attempts, newest first (at most [`METRICS_RECENT_RUNS`]).
    pub recent_runs: Vec<MetricsRunView>,
}

/// Health of every spec.
///
/// # Errors
///
/// Store failures.
pub fn metrics_health(
    reader: &StoreReader,
    specs: &[MetricsSpec],
) -> Result<Vec<MetricsHealth>, QueryError> {
    specs
        .iter()
        .map(|spec| {
            let runs = reader.metrics_runs(&spec.id, METRICS_HEALTH_RUNS)?;
            Ok(MetricsHealth {
                output: spec.id.clone(),
                kind: spec.kind,
                state: metrics_state(&runs),
                last_success_at: runs.iter().find(|r| r.error.is_none()).map(|r| r.finished),
                recent_runs: runs
                    .iter()
                    .take(METRICS_RECENT_RUNS)
                    .map(MetricsRunView::from)
                    .collect(),
            })
        })
        .collect()
}
