//! Loading the config, applying a new one (D22) and arming the jobs it enables.

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use camino::Utf8Path;
use tokenmtr_agents::AgentSource;
use tokenmtr_agents::ingest::{IngestSettings, Ingestor};
use tokenmtr_app::{load_config, store_path};
use tokenmtr_config::Config;
use tokenmtr_platform::ThreadPriority;
use tokenmtr_store::StoreRead;

use crate::changes::std_duration;
use crate::ctx::{Active, Ctx, Job, WatchPlan, lock};
use crate::error::DaemonError;

/// What the watcher should watch for `config`; empty when `daemon.watch` is off.
pub(crate) fn watch_plan(
    config: &Config,
    ingest: &IngestSettings,
    ingestor: &Ingestor,
) -> WatchPlan {
    if !config.daemon.watch {
        return WatchPlan::default();
    }
    WatchPlan {
        roots: ingestor.roots(),
        owner: Some(ingest.clone()),
    }
}

/// Whether some agent of `settings` owns `path` (the same test as `Ingestor::owns`).
pub(crate) fn owns(settings: &IngestSettings, path: &Utf8Path) -> bool {
    settings
        .sources
        .iter()
        .any(|source| source.owns(path) && source.profiles().iter().any(|p| p.contains(path)))
}

impl Ctx {
    /// Reloads the config file (D22). An invalid file keeps the config in force and is recorded
    /// as the `config_error` of status and issues; a valid one replaces it everywhere.
    ///
    /// # Errors
    /// [`DaemonError::Config`] when the new config is invalid, [`DaemonError::Ingest`] when the
    /// new ingestor cannot start.
    pub(crate) async fn reload(self: &Arc<Self>) -> Result<(), DaemonError> {
        let result = self
            .blocking(ThreadPriority::UserInitiated, Self::reload_blocking)
            .await?;
        if let Err(error) = &result {
            tracing::warn!(%error, "keeping the previous configuration");
            lock(&self.run).config_error = Some(error.to_string());
        }
        result
    }

    fn reload_blocking(&self) -> Result<(), DaemonError> {
        let config = load_config(&self.settings.config_path, &*self.settings.env)?;
        let ingest =
            IngestSettings::from_config(&config, &self.settings.paths, &*self.settings.env);
        let ingestor = Ingestor::open(ingest.clone(), &self.store_path)?;
        let plan = watch_plan(&config, &ingest, &ingestor);
        let sinks = Self::build_sinks(&self.settings.paths, &config);
        let old = self.active();
        if store_path(&self.settings.paths, &config) != self.store_path {
            tracing::warn!("`store.path` changed: restart tokenmtr to use the new store");
        }
        let new = Arc::new(Active { config, ingest });
        {
            let mut current = lock(&self.ingestor);
            let mut exporters = lock(&self.sinks);
            *current = ingestor;
            *exporters = sinks;
            *self.active.write().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&new);
        }
        lock(&self.run).config_error = None;
        self.sync_subscriptions(&new.config);
        self.watch_plan.send_replace(plan);
        self.rearm(Some(&old), &new);
        tracing::info!("configuration reloaded");
        Ok(())
    }

    /// Writes the configured subscriptions (D35) into the store.
    pub(crate) fn sync_subscriptions(&self, config: &Config) {
        if let Err(error) = lock(&self.store).upsert_subscriptions(&config.subscriptions) {
            tracing::warn!(%error, "cannot store the subscriptions");
        }
        self.after_write(false);
    }

    /// Plans the jobs `new` enables and drops those it disables. With `old`, only what changed is
    /// touched, so a reload does not restart healthy timers.
    pub(crate) fn rearm(&self, old: Option<&Active>, new: &Active) {
        let (c, o) = (&new.config, old.map(|old| &old.config));
        let changed =
            |period: fn(&Config) -> Option<Duration>| o.is_none_or(|o| period(o) != period(c));

        let sweep = |config: &Config| Some(std_duration(config.daemon.scan_interval));
        if changed(sweep) {
            let period = std_duration(c.daemon.scan_interval);
            self.scheduler.cancel(Job::Sweep);
            // The startup rescan is the watcher task's (D16); the sweep is the safety net.
            self.scheduler
                .schedule_in(Job::Sweep, period, self.slack(period));
        }

        let backup = |config: &Config| {
            config
                .backup
                .enabled
                .then(|| std_duration(config.backup.interval))
        };
        if changed(backup) {
            self.scheduler.cancel(Job::Backup);
            if c.backup.enabled {
                let due = self.backup_due_in();
                // Slack relative to the wait, not the interval: a 60 s first-backup delay must
                // not be batched into the startup wakeup.
                self.scheduler
                    .schedule_in(Job::Backup, due, self.slack(due));
            }
        }

        let quota = |config: &Config| {
            config
                .quota
                .enabled
                .then(|| std_duration(config.quota.interval))
        };
        if changed(quota) {
            self.scheduler.cancel(Job::Quota);
            if c.quota.enabled {
                let due = if o.is_some_and(|o| o.quota.enabled) {
                    std_duration(c.quota.interval)
                } else {
                    Duration::ZERO
                };
                self.scheduler.schedule_in(
                    Job::Quota,
                    due,
                    self.slack(std_duration(c.quota.interval)),
                );
            }
        }

        let fx = |config: &Config| {
            (config.fx.enabled && config.fx.fixed_eur_per_usd.is_none()).then_some(Duration::ZERO)
        };
        if changed(fx) {
            self.scheduler.cancel(Job::Fx);
            if fx(c).is_some() {
                self.scheduler
                    .schedule_in(Job::Fx, Duration::ZERO, Duration::ZERO);
            }
        }

        let pricing = |config: &Config| config.pricing.auto_update.then_some(Duration::ZERO);
        if changed(pricing) {
            self.scheduler.cancel(Job::Pricing);
            if c.pricing.auto_update {
                self.scheduler
                    .schedule_in(Job::Pricing, self.pricing_due_in(), Duration::ZERO);
            }
        }

        for sink in &c.sinks {
            if o.is_none_or(|o| !o.sinks.iter().any(|s| s.id == sink.id)) {
                self.schedule_now(Job::Export(sink.id.clone()));
            }
        }
        for sink in o.into_iter().flat_map(|o| &o.sinks) {
            if !c.sinks.iter().any(|s| s.id == sink.id) {
                self.scheduler.cancel(Job::Export(sink.id.clone()));
            }
        }
    }

    /// How long until the price list is due: a day after the last download of a snapshot this
    /// daemon fetched, immediately otherwise.
    fn pricing_due_in(&self) -> Duration {
        let snapshot = lock(&self.reader).active_pricing_snapshot().ok().flatten();
        let Some(snapshot) =
            snapshot.filter(|s| s.source.as_deref() == Some(crate::refresh::PRICING_SOURCE))
        else {
            return Duration::ZERO;
        };
        let age = jiff::Timestamp::now().duration_since(snapshot.fetched_at);
        let age = Duration::try_from(age).unwrap_or(Duration::ZERO);
        self.settings.timing.pricing_interval.saturating_sub(age)
    }
}
