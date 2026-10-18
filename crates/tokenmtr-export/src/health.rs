//! Sink health, a pure function of the run history and what is still pending.
//!
//! The caller gathers the inputs: recent `sink_run` rows of the sink and, per dataset, the
//! [`Pending`] rows past the sink's cursor from `StoreRead::pending`.

use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use tokenmtr_store::{Pending, SinkRun};

/// A sink is lagging when its oldest unexported row is older than this many intervals.
pub const LAG_INTERVALS: i32 = 2;

/// How many of a sink's newest runs to pass as [`HealthInput::runs`]: a failure streak longer
/// than this reports the oldest run of the window as its start.
pub const HEALTH_RUNS: usize = 200;

/// How a sink is doing. Precedence: `Disabled`, then `Failing`, `Lagging`, `Healthy`.
/// Serialises as `{"state": "healthy" | "lagging" | "failing" | "disabled", ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SinkState {
    /// Delivering on time.
    Healthy,
    /// Delivering, but the oldest pending row is older than [`LAG_INTERVALS`] intervals.
    Lagging,
    /// The latest run failed.
    Failing {
        /// Start of the first run of the current streak of failures.
        since: Timestamp,
        /// Error of the latest failed run.
        last_error: String,
    },
    /// Turned off by the user; never runs.
    Disabled,
}

impl SinkState {
    /// Severity for "worst of" aggregation: higher is worse.
    #[must_use]
    pub const fn severity(&self) -> u8 {
        match self {
            Self::Disabled | Self::Healthy => 0,
            Self::Lagging => 1,
            Self::Failing { .. } => 2,
        }
    }
}

/// How far behind the store a sink is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkLag {
    /// Rows past the cursors.
    pub rows: u64,
    /// Age of the oldest of them, `None` when nothing is pending.
    pub age: Option<SignedDuration>,
}

/// Health plus lag, what the recap and `doctor` show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkStatus {
    /// The state.
    pub state: SinkState,
    /// Lag in rows and time.
    pub lag: SinkLag,
}

/// Everything [`sink_status`] needs.
#[derive(Debug, Clone, Copy)]
pub struct HealthInput<'a> {
    /// False for a disabled sink.
    pub enabled: bool,
    /// The sink's configured interval.
    pub interval: SignedDuration,
    /// The current time.
    pub now: Timestamp,
    /// Recent runs of the sink, any order.
    pub runs: &'a [SinkRun],
    /// Rows past the cursors, summed over the sink's datasets with [`total_pending`].
    pub pending: Pending,
}

/// Adds up per-dataset [`Pending`]: rows are summed, the oldest instant is the earliest.
#[must_use]
pub fn total_pending(parts: impl IntoIterator<Item = Pending>) -> Pending {
    parts.into_iter().fold(
        Pending {
            rows: 0,
            oldest: None,
        },
        |total, part| Pending {
            rows: total.rows.saturating_add(part.rows),
            oldest: match (total.oldest, part.oldest) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        },
    )
}

/// Computes a sink's state and lag.
#[must_use]
pub fn sink_status(input: &HealthInput<'_>) -> SinkStatus {
    let age = input
        .pending
        .oldest
        .map(|oldest| input.now.duration_since(oldest).max(SignedDuration::ZERO));
    let lag = SinkLag {
        rows: input.pending.rows,
        age,
    };
    let health = if !input.enabled {
        SinkState::Disabled
    } else if let Some(failing) = failing(input.runs) {
        failing
    } else if age.is_some_and(|age| age > input.interval.saturating_mul(LAG_INTERVALS)) {
        SinkState::Lagging
    } else {
        SinkState::Healthy
    };
    SinkStatus { state: health, lag }
}

fn failing(runs: &[SinkRun]) -> Option<SinkState> {
    let mut sorted: Vec<&SinkRun> = runs.iter().collect();
    sorted.sort_by_key(|run| run.started);
    let latest = sorted.last()?;
    let last_error = latest.error.clone()?;
    let since = sorted
        .iter()
        .rev()
        .take_while(|run| run.error.is_some())
        .last()
        .map_or(latest.started, |run| run.started);
    Some(SinkState::Failing { since, last_error })
}
