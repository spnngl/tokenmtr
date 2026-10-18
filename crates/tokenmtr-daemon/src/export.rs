//! Exporting to sinks (D47): one event-driven job per sink, with backoff on failure.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use tokenmtr_core::SinkId;
use tokenmtr_export::{
    HealthInput, PumpOptions, SinkHealth, SinkRunOutcome, export_pending, sink_status,
};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_store::{Pending, StoreRead};
use tokio::time::Instant;

use crate::changes::std_duration;
use crate::ctx::{Ctx, Job, lock};
use crate::error::DaemonError;

/// How many past runs decide when a failure streak began.
const HISTORY: usize = 200;

/// What one sink's run did.
#[derive(Debug, Clone)]
pub(crate) struct SinkOutcome {
    pub sink: SinkId,
    pub rows: u64,
    pub error: Option<String>,
}

impl From<&SinkRunOutcome> for SinkOutcome {
    fn from(outcome: &SinkRunOutcome) -> Self {
        Self {
            sink: outcome.sink.clone(),
            rows: outcome.rows(),
            error: outcome.error.as_ref().map(ToString::to_string),
        }
    }
}

impl Ctx {
    /// Exports `only` (every sink when `None`) at `priority`.
    pub(crate) async fn export(
        self: &Arc<Self>,
        only: Option<SinkId>,
        priority: ThreadPriority,
    ) -> Result<Vec<SinkOutcome>, DaemonError> {
        self.blocking(priority, move |ctx| ctx.export_blocking(only.as_ref()))
            .await
    }

    /// Exports `only` (every sink when `None`), records each outcome and plans retries.
    pub(crate) fn export_blocking(&self, only: Option<&SinkId>) -> Vec<SinkOutcome> {
        let outcomes: Vec<SinkOutcome> = {
            let mut sinks = lock(&self.sinks);
            let mut store = lock(&self.store);
            sinks
                .iter_mut()
                .filter(|sink| only.is_none_or(|id| *id == sink.spec.id))
                .flat_map(|sink| {
                    export_pending(
                        &mut store,
                        std::slice::from_mut(sink),
                        &PumpOptions::default(),
                    )
                })
                .map(|outcome| SinkOutcome::from(&outcome))
                .collect()
        };
        for outcome in &outcomes {
            self.note_export(outcome);
        }
        self.evaluate_alerts(crate::alerts::Evaluation::default());
        outcomes
    }

    /// Updates what is known of the sink and plans its next attempt on failure.
    fn note_export(&self, outcome: &SinkOutcome) {
        let active = self.active();
        let Some(config) = active
            .config
            .sinks
            .iter()
            .find(|sink| sink.id == outcome.sink)
        else {
            return;
        };
        let now = Timestamp::now();
        let Some(error) = &outcome.error else {
            let mut run = lock(&self.run);
            let runtime = run.sinks.entry(outcome.sink.clone()).or_default();
            runtime.last_attempt = Some(Instant::now());
            runtime.failures = 0;
            runtime.last_ok_at = Some(now);
            runtime.last_error = None;
            runtime.failing_since = None;
            return;
        };
        tracing::warn!(sink = %outcome.sink, %error, "export failed");
        let known = lock(&self.run)
            .sinks
            .get(&outcome.sink)
            .and_then(|runtime| runtime.failing_since);
        let since = known.unwrap_or_else(|| self.failure_start(config, now));
        let interval = std_duration(config.interval);
        let failures = {
            let mut run = lock(&self.run);
            let runtime = run.sinks.entry(outcome.sink.clone()).or_default();
            runtime.last_attempt = Some(Instant::now());
            runtime.failures = runtime.failures.saturating_add(1);
            runtime.last_error = Some(error.clone());
            runtime.failing_since = Some(since);
            runtime.failures
        };
        let mut delay = self.retry_delay(interval, failures);
        // The `sink_failing` alert must fire when its threshold passes, not at the next backoff.
        let threshold = active.config.alerts.rules.sink_failing_after;
        let remaining = since
            .checked_add(threshold)
            .map(|due| due.duration_since(now))
            .ok()
            .and_then(|remaining| Duration::try_from(remaining).ok())
            .filter(|remaining| !remaining.is_zero());
        if let Some(remaining) = remaining {
            delay = delay.min(remaining.max(Duration::from_millis(100)));
        }
        self.scheduler
            .schedule_in(Job::Export(outcome.sink.clone()), delay, Duration::ZERO);
    }

    /// Start of the failure streak of the sink according to its run history; `now` when the
    /// history does not show one.
    fn failure_start(&self, config: &tokenmtr_config::SinkConfig, now: Timestamp) -> Timestamp {
        let runs = lock(&self.reader)
            .sink_runs(&config.id, HISTORY)
            .unwrap_or_default();
        let status = sink_status(&HealthInput {
            enabled: true,
            interval: config.interval,
            now,
            runs: &runs,
            pending: Pending {
                rows: 0,
                oldest: None,
            },
        });
        match status.health {
            SinkHealth::Failing { since, .. } => since,
            _ => now,
        }
    }

    /// Wait before the next attempt of a sink that failed `failures` times in a row: the sink
    /// interval doubled each time, capped.
    fn retry_delay(&self, interval: Duration, failures: u32) -> Duration {
        let base = interval.max(self.settings.timing.min_slack.min(Duration::from_secs(1)));
        let doublings = failures.saturating_sub(1).min(16);
        base.saturating_mul(1_u32 << doublings)
            .min(self.settings.timing.export_retry_cap)
    }
}
