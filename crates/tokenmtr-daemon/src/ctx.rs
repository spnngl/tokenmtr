//! The state every daemon task shares, and the helpers around it.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use camino::Utf8PathBuf;
use jiff::Timestamp;
use tokenmtr_agents::ingest::{IngestSettings, Ingestor};
use tokenmtr_config::Config;
use tokenmtr_control::{Dataset as WireDataset, EventBus};
use tokenmtr_core::{AgentId, ProfileId, SinkId, WindowId};
use tokenmtr_export::{ConfiguredSink, SinkSpec};
use tokenmtr_platform::{Paths, ThreadPriority, set_thread_priority};
use tokenmtr_remote::{Endpoints, Validators, build_client};
use tokenmtr_store::{Dataset, Revision, Store, StoreRead, StoreReader};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use crate::error::DaemonError;
use crate::scheduler::Scheduler;
use crate::settings::Settings;

/// One unit of scheduled work (D47).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Job {
    /// Write the heartbeat into the store.
    Heartbeat,
    /// Scan every agent file: the periodic sweep (D16).
    Sweep,
    /// Write a backup (D37).
    Backup,
    /// Poll Claude quota (D26).
    Quota,
    /// Refresh ECB rates (D27).
    Fx,
    /// Refresh the price list (D11).
    Pricing,
    /// Export pending rows to one sink (D47).
    Export(SinkId),
    /// Push coalesced `StoreChanged` events to subscribers (D47).
    Publish,
    /// Reload the config file (D22).
    Reload,
}

/// The config in force and what was derived from it once.
#[derive(Debug)]
pub(crate) struct Active {
    pub config: Config,
    pub ingest: IngestSettings,
}

/// What the file watcher should watch.
#[derive(Debug, Clone, Default)]
pub(crate) struct WatchPlan {
    /// Agent roots; empty when `daemon.watch` is off.
    pub roots: Vec<Utf8PathBuf>,
    /// Decides which changed paths an agent owns; `None` when `daemon.watch` is off.
    pub owner: Option<IngestSettings>,
}

/// The network clients, built when the first network task runs.
pub(crate) struct Http {
    pub client: reqwest::Client,
    pub endpoints: Endpoints,
}

/// What the daemon knows about one sink's recent attempts.
#[derive(Debug, Default, Clone)]
pub(crate) struct SinkRuntime {
    pub last_attempt: Option<Instant>,
    /// Consecutive failed attempts; 0 after a success.
    pub failures: u32,
    pub last_ok_at: Option<Timestamp>,
    pub last_error: Option<String>,
    /// Start of the current failure streak; the same value until the sink recovers, so the
    /// `sink_failing` dedup key stays stable.
    pub failing_since: Option<Timestamp>,
}

/// Mutable facts that live only as long as the process.
#[derive(Debug, Default)]
pub(crate) struct Runtime {
    pub heads: HashMap<Dataset, Revision>,
    pub pending_changed: BTreeSet<WireDataset>,
    pub last_publish: Option<Instant>,
    pub last_scan_at: Option<Timestamp>,
    pub config_error: Option<String>,
    pub sinks: HashMap<SinkId, SinkRuntime>,
    pub quota_resets: HashMap<(ProfileId, WindowId), Option<Timestamp>>,
    pub account_checked: HashMap<(AgentId, ProfileId), Timestamp>,
    pub pricing_etag: Option<String>,
    pub fx_validators: Validators,
    pub backup_failed_at: Option<Timestamp>,
}

/// Shared by every task; always behind an [`Arc`].
///
/// Lock order, outermost first: `ingestor`, `sinks`, `store`. `reader`, `run` and `active` are
/// leaf locks: never hold one while taking another lock.
pub(crate) struct Ctx {
    pub settings: Settings,
    pub store_path: Utf8PathBuf,
    /// The single writer (D1).
    pub store: Mutex<Store>,
    /// A second connection: queries, backups and head checks never wait for a write.
    pub reader: Mutex<StoreReader>,
    pub ingestor: Mutex<Ingestor>,
    pub sinks: Mutex<Vec<ConfiguredSink>>,
    pub active: RwLock<Arc<Active>>,
    pub run: Mutex<Runtime>,
    pub bus: EventBus,
    pub scheduler: Scheduler<Job>,
    pub watch_plan: watch::Sender<WatchPlan>,
    pub http: OnceLock<Http>,
    pub started_at: Timestamp,
    /// Notified by `Request::Shutdown`; the process owner calls `Daemon::shutdown` then.
    pub stop_requested: Notify,
}

/// Locks `mutex`; a panic in another holder does not make the daemon unusable.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Ctx {
    /// The config in force.
    pub(crate) fn active(&self) -> Arc<Active> {
        Arc::clone(&self.active.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Slack of a periodic job (D47): 10% of its period, at least [`Timing::min_slack`].
    ///
    /// [`Timing::min_slack`]: crate::Timing::min_slack
    pub(crate) fn slack(&self, period: Duration) -> Duration {
        (period / 10).max(self.settings.timing.min_slack)
    }

    /// Plans `job` `delay` from now with the slack of `period`.
    pub(crate) fn schedule_periodic(&self, job: Job, period: Duration) {
        self.scheduler.schedule_in(job, period, self.slack(period));
    }

    /// Plans `job` at once.
    pub(crate) fn schedule_now(&self, job: Job) {
        self.scheduler
            .schedule_in(job, Duration::ZERO, Duration::ZERO);
    }

    /// The network clients, built on first use.
    pub(crate) fn http(&self) -> Result<&Http, DaemonError> {
        if let Some(http) = self.http.get() {
            return Ok(http);
        }
        let endpoints = match &self.settings.endpoints {
            Some(endpoints) => endpoints.clone(),
            None => Endpoints::production()?,
        };
        let http = Http {
            client: build_client()?,
            endpoints,
        };
        Ok(self.http.get_or_init(|| http))
    }

    /// Runs `work` on a blocking thread at `priority`, then puts the thread back to background
    /// priority (D47).
    pub(crate) async fn blocking<T, F>(
        self: &Arc<Self>,
        priority: ThreadPriority,
        work: F,
    ) -> Result<T, DaemonError>
    where
        F: FnOnce(&Self) -> T + Send + 'static,
        T: Send + 'static,
    {
        let ctx = Arc::clone(self);
        let out = tokio::task::spawn_blocking(move || {
            set_priority(priority);
            let out = work(&ctx);
            set_priority(ThreadPriority::Background);
            out
        })
        .await?;
        Ok(out)
    }

    /// The sinks of `config`, ready to write.
    pub(crate) fn build_sinks(paths: &Paths, config: &Config) -> Vec<ConfiguredSink> {
        let exports = paths.exports_dir();
        config
            .sinks
            .iter()
            .map(|sink| ConfiguredSink::new(SinkSpec::from_config(sink, &exports)))
            .collect()
    }

    /// The head revision of every dataset now.
    pub(crate) fn heads(&self) -> Result<HashMap<Dataset, Revision>, DaemonError> {
        let reader = lock(&self.reader);
        Dataset::ALL
            .into_iter()
            .map(|dataset| Ok((dataset, reader.head_revision(dataset)?)))
            .collect()
    }
}

fn set_priority(priority: ThreadPriority) {
    if let Err(error) = set_thread_priority(priority) {
        tracing::debug!(%error, "cannot set the thread priority");
    }
}
