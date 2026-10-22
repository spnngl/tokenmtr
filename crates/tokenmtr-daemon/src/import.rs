//! Importing other machines' exports: one job per enabled `sqlite` source, on its interval and
//! when a file it includes changes.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokenmtr_config::{Config, SourceConfig, SourceTarget};
use tokenmtr_core::SourceId;
use tokenmtr_import::{ImportOptions, ImportReport, ImportSpec, import_pending};
use tokenmtr_platform::ThreadPriority;

use crate::alerts::Evaluation;
use crate::changes::std_duration;
use crate::ctx::{Ctx, Job, lock};
use crate::error::DaemonError;

/// The `sqlite` sources `config` reads: enabled ones only.
pub(crate) fn enabled_imports(config: &Config) -> impl Iterator<Item = &SourceConfig> {
    config
        .sources
        .iter()
        .filter(|source| source.enabled && matches!(source.target, SourceTarget::Sqlite(_)))
}

impl Ctx {
    /// The period of `source` when the config in force still wants it imported.
    pub(crate) fn import_interval(&self, source: &SourceId) -> Option<Duration> {
        let active = self.active();
        enabled_imports(&active.config)
            .find(|candidate| candidate.id == *source)
            .and_then(|candidate| match &candidate.target {
                SourceTarget::Sqlite(sqlite) => Some(std_duration(sqlite.interval)),
                SourceTarget::Superset(_) => None,
            })
    }

    /// The `Import` job: one pass over `source`, then its next timer; a source the config no
    /// longer enables is dropped.
    pub(crate) async fn run_import(self: &Arc<Self>, source: SourceId) {
        let Some(interval) = self.import_interval(&source) else {
            return;
        };
        if let Err(error) = self
            .import(Some(source.clone()), ThreadPriority::Background)
            .await
        {
            tracing::warn!(%error, "import task failed");
        }
        self.schedule_periodic(Job::Import(source), interval);
    }

    /// Imports `only` (every enabled `sqlite` source when `None`) at `priority`.
    pub(crate) async fn import(
        self: &Arc<Self>,
        only: Option<SourceId>,
        priority: ThreadPriority,
    ) -> Result<Vec<ImportReport>, DaemonError> {
        self.blocking(priority, move |ctx| ctx.import_blocking(only.as_ref()))
            .await
    }

    /// Imports `only` (every enabled `sqlite` source when `None`) on the `imports` connection,
    /// so scans and the heartbeat keep writing while a slow folder is read. A pass stops between
    /// pages when the daemon shuts down.
    pub(crate) fn import_blocking(&self, only: Option<&SourceId>) -> Vec<ImportReport> {
        let active = self.active();
        let redact_paths = active.config.privacy.redact_paths;
        let specs: Vec<ImportSpec> = enabled_imports(&active.config)
            .filter(|source| only.is_none_or(|id| *id == source.id))
            .filter_map(|source| ImportSpec::from_config(source, redact_paths))
            .collect();
        let stopping = || self.stopping.load(Ordering::Relaxed);
        let reports: Vec<ImportReport> = {
            let mut store = lock(&self.imports);
            specs
                .iter()
                .map(|spec| import_pending(&mut store, spec, &ImportOptions::default(), &stopping))
                .collect()
        };
        for report in &reports {
            for failure in &report.failures {
                tracing::warn!(source = %report.source, path = %failure.path, error = %failure.error, "import failed");
            }
        }
        if reports.iter().any(|report| report.rows() > 0) {
            // Imported rows move the heads and count in budgets.
            self.after_write(true);
        } else if reports.iter().any(|report| !report.failures.is_empty()) {
            // A streak of failures may just have outlived `source_failing_after`.
            self.evaluate_alerts(Evaluation::default());
        }
        reports
    }

    /// Plans the imports `new` enables and drops those it disables. With `old`, a source whose
    /// settings did not change keeps its timer.
    pub(crate) fn arm_imports(&self, old: Option<&Config>, new: &Config) {
        for source in enabled_imports(new) {
            let unchanged = old.is_some_and(|old| old.sources.contains(source));
            if !unchanged {
                self.scheduler.cancel(Job::Import(source.id.clone()));
                self.schedule_now(Job::Import(source.id.clone()));
            }
        }
        for source in old.into_iter().flat_map(enabled_imports) {
            if !enabled_imports(new).any(|candidate| candidate.id == source.id) {
                self.scheduler.cancel(Job::Import(source.id.clone()));
            }
        }
    }
}
