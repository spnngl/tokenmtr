//! Reacting to store writes: notify subscribers, wake the sinks, evaluate alerts.

use std::collections::BTreeSet;
use std::time::Duration;

use jiff::SignedDuration;
use tokenmtr_control::Event;
use tokenmtr_store::Dataset;
use tokio::time::Instant;

use crate::alerts::Evaluation;
use crate::ctx::{Ctx, Job, lock};
use crate::error::DaemonError;

/// A config duration as a std one; a negative value is no wait.
pub(crate) fn std_duration(duration: SignedDuration) -> Duration {
    Duration::try_from(duration).unwrap_or(Duration::ZERO)
}

impl Ctx {
    /// Call after every store write: pushes `StoreChanged` when a head revision moved, wakes the
    /// sinks that export what changed, and evaluates the alerts. `budgets` asks for the
    /// budget rules too; they query usage, so only ingest needs them.
    pub(crate) fn after_write(&self, budgets: bool) {
        match self.detect_changes() {
            Ok(changed) if !changed.is_empty() => self.on_changed(&changed),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "cannot read the store's revisions"),
        }
        self.evaluate_alerts(Evaluation {
            quota: Vec::new(),
            budgets,
        });
    }

    /// The datasets whose head revision moved since the last call.
    fn detect_changes(&self) -> Result<BTreeSet<Dataset>, DaemonError> {
        let heads = self.heads()?;
        let mut run = lock(&self.run);
        let changed = heads
            .iter()
            .filter(|(dataset, revision)| run.heads.get(dataset) != Some(revision))
            .map(|(dataset, _)| *dataset)
            .collect();
        run.heads = heads;
        Ok(changed)
    }

    fn on_changed(&self, changed: &BTreeSet<Dataset>) {
        let now = Instant::now();
        let coalesce = self.settings.timing.event_coalesce;
        let publish_at = {
            let mut run = lock(&self.run);
            run.pending_changed.extend(changed.iter().copied());
            run.last_publish.map_or(now, |at| (at + coalesce).max(now))
        };
        self.scheduler
            .schedule(Job::Publish, publish_at, Duration::ZERO);

        self.wake_metrics(changed, now);

        let active = self.active();
        for sink in &active.config.sinks {
            if !sink
                .datasets
                .iter()
                .any(|dataset| changed.contains(dataset))
            {
                continue;
            }
            let due = {
                let mut run = lock(&self.run);
                let runtime = run.sinks.entry(sink.id.clone()).or_default();
                if runtime.failures > 0 {
                    // The retry timer of the failing sink decides; new rows do not skip the
                    // backoff.
                    continue;
                }
                runtime
                    .last_attempt
                    .map_or(now, |at| (at + std_duration(sink.interval)).max(now))
            };
            self.scheduler
                .schedule(Job::Export(sink.id.clone()), due, Duration::ZERO);
        }
    }

    /// Pushes the pending `StoreChanged` to subscribers, if anything changed (the `Publish` job).
    pub(crate) fn publish(&self) {
        let datasets: Vec<Dataset> = {
            let mut run = lock(&self.run);
            if run.pending_changed.is_empty() {
                return;
            }
            run.last_publish = Some(Instant::now());
            std::mem::take(&mut run.pending_changed)
                .into_iter()
                .collect()
        };
        self.bus.publish(Event::StoreChanged { datasets });
    }
}
