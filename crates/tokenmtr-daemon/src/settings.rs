//! What the process hands the daemon: where things are, and how fast it runs.

use std::sync::Arc;
use std::time::Duration;

use camino::Utf8PathBuf;
use tokenmtr_agents::ingest::watch::Debounce;
use tokenmtr_config::{Env, ProcessEnv};
use tokenmtr_platform::{ControlTransport, Paths, SecretStore};
use tokenmtr_remote::Endpoints;

/// Delays of the daemon's own machinery. The defaults are the production values; tests shorten
/// them. Config values (scan interval, sink intervals, ...) are not here: they come from the
/// config file.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// How often the heartbeat is written into the store.
    pub heartbeat: Duration,
    /// Debounce of file events.
    pub watch: Debounce,
    /// How often the watcher checks whether a missing agent root appeared, to start watching it.
    /// It only wakes for this while some root is missing.
    pub root_poll: Duration,
    /// `StoreChanged` is pushed to subscribers at most this often.
    pub event_coalesce: Duration,
    /// A config file event triggers a reload this long after the first event.
    pub reload_debounce: Duration,
    /// A file event in a source folder triggers its import this long after the first event:
    /// a sync client writes a file in several steps.
    pub import_debounce: Duration,
    /// Smallest slack of a periodic job (10% of its period, at least this): jobs due within it
    /// share one wakeup.
    pub min_slack: Duration,
    /// Longest wait between two attempts of a failing sink.
    pub export_retry_cap: Duration,
    /// First wait after a failed bind or push of a metrics output; it doubles on each failure
    /// (up to `export_retry_cap` for a bind, the push interval for a push).
    pub metrics_backoff: Duration,
    /// How often the price list is refreshed.
    pub pricing_interval: Duration,
    /// How often ECB rates are refreshed.
    pub fx_interval: Duration,
    /// How long to wait before retrying a failed FX or price list download.
    pub network_retry: Duration,
    /// When the first backup runs if none exists yet.
    pub first_backup_delay: Duration,
    /// How long shutdown waits for running jobs and the last export before giving up on them.
    /// With the CLI's runtime grace it must stay under the 2 s stop budget.
    pub shutdown_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(60),
            watch: Debounce::default(),
            root_poll: Duration::from_secs(60),
            event_coalesce: Duration::from_secs(1),
            reload_debounce: Duration::from_millis(500),
            import_debounce: Duration::from_secs(2),
            min_slack: Duration::from_secs(1),
            export_retry_cap: Duration::from_secs(300),
            metrics_backoff: Duration::from_secs(1),
            pricing_interval: Duration::from_hours(24),
            fx_interval: Duration::from_hours(24),
            network_retry: Duration::from_hours(1),
            first_backup_delay: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(1200),
        }
    }
}

/// Everything [`Daemon::start`](crate::Daemon::start) needs from its environment.
pub struct Settings {
    /// Platform directories.
    pub paths: Paths,
    /// The config file; a missing file means the defaults.
    pub config_path: Utf8PathBuf,
    /// Where the control socket lives.
    pub transport: ControlTransport,
    /// Where OAuth credentials come from.
    pub secrets: Arc<dyn SecretStore + Send + Sync>,
    /// The environment agents read their directories from.
    pub env: Arc<dyn Env + Send + Sync>,
    /// Hosts of the network clients; `None` is production, resolved only when a network task
    /// first runs, so a daemon with every network task off builds no client at all.
    pub endpoints: Option<Endpoints>,
    /// Delays of the daemon's own machinery.
    pub timing: Timing,
}

impl Settings {
    /// Production settings over `paths`: the process environment, production hosts, default
    /// timing.
    #[must_use]
    pub fn new(
        paths: Paths,
        config_path: Utf8PathBuf,
        transport: ControlTransport,
        secrets: Arc<dyn SecretStore + Send + Sync>,
    ) -> Self {
        Self {
            paths,
            config_path,
            transport,
            secrets,
            env: Arc::new(ProcessEnv),
            endpoints: None,
            timing: Timing::default(),
        }
    }
}
