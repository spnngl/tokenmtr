//! Starting and stopping the daemon, and the job runner.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use jiff::Timestamp;
use tokenmtr_agents::ingest::{IngestSettings, Ingestor, ScanPriority, ScanRequest};
use tokenmtr_control::{EventBus, Server, ServerInfo, Shutdown};
use tokenmtr_core::{AgentId, ProfileId, WindowId};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_platform::control::DaemonLock;
use tokenmtr_store::{
    QuotaReading, Revision, SCHEMA_VERSION, Store, StoreRead, StoreReader, Versioned,
};
use tokio::sync::{Notify, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

use crate::changes::std_duration;
use crate::ctx::{Active, Ctx, Job, QuotaResets, Runtime, lock};
use crate::error::DaemonError;
use crate::handler::ControlHandler;
use crate::reload::{ReloadCause, config_bytes, watch_plan};
use crate::scheduler::Scheduler;
use crate::settings::Settings;
use crate::watcher;
use tokenmtr_app::{load_config, store_path};

/// A running daemon: the control server, the scheduler and the watcher, all inside the current
/// tokio runtime.
pub struct Daemon {
    ctx: Arc<Ctx>,
    stop: watch::Sender<bool>,
    tasks: JoinSet<()>,
    /// The control server; hands back the daemon lock once the socket is gone.
    server: JoinHandle<DaemonLock>,
}

impl Daemon {
    /// Starts the daemon: takes the daemon lock and binds the control socket (`0600`), loads the
    /// config, opens the store and spawns every task. Returns once the socket accepts
    /// connections. The lock is held until [`Daemon::shutdown`] ends.
    ///
    /// # Errors
    /// [`DaemonError::AlreadyRunning`] when another daemon owns the socket,
    /// [`DaemonError::Locked`] when another process (an erase, a restore) holds the lock,
    /// [`DaemonError::NewerSchema`] when the store was written by a newer `TokenMtr` (it is left
    /// untouched), [`DaemonError::Config`] when the config is invalid; store and I/O failures
    /// otherwise.
    pub async fn start(settings: Settings) -> Result<Self, DaemonError> {
        // Lock and bind first, so a second daemon fails before it touches the store. On
        // error the listener drops, removing the socket and releasing the lock.
        let listener = settings.transport.bind().await?;
        let socket = settings.transport.socket_path().to_owned();
        let (ctx, commands, server_bus) = build(settings).await?;

        let (stop, shutdown) = watch::channel(false);
        let mut tasks = JoinSet::new();
        tasks.spawn(scheduler_task(Arc::clone(&ctx), commands, shutdown.clone()));
        tasks.spawn(watcher::run(Arc::clone(&ctx), shutdown.clone()));
        let server = Server::new(
            listener,
            ControlHandler(Arc::clone(&ctx)),
            ServerInfo {
                daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                store_schema: SCHEMA_VERSION,
            },
            server_bus,
        );
        let server = tokio::spawn(server.serve(shutdown));

        ctx.schedule_now(Job::Heartbeat);
        // `rearm` reads the store and the backup directory.
        let armed = ctx
            .blocking(ThreadPriority::UserInitiated, |ctx| {
                ctx.rearm(None, &ctx.active());
            })
            .await;
        if let Err(error) = armed {
            tracing::error!(%error, "cannot schedule the periodic jobs");
        }
        tracing::info!(socket = %socket, "daemon started");
        Ok(Self {
            ctx,
            stop,
            tasks,
            server,
        })
    }

    /// Resolves once a client sent `Request::Shutdown`. The owner of the daemon then calls
    /// [`Daemon::shutdown`], as it does on SIGTERM. Cancel-safe; a request made before the call is
    /// not lost.
    pub async fn stop_requested(&self) {
        self.ctx.stop_requested.notified().await;
    }

    /// Stops the daemon: no new work starts, the socket is removed, the running transaction
    /// finishes, sinks get a last export, then the daemon lock is released. Takes at most
    /// [`Timing::shutdown_grace`]; when running work does not stop by then it is abandoned and
    /// the last export is skipped, since it would wait on the locks that work holds.
    ///
    /// # Errors
    /// [`DaemonError::Task`] when the control server panicked.
    ///
    /// [`Timing::shutdown_grace`]: crate::Timing::shutdown_grace
    pub async fn shutdown(mut self) -> Result<(), DaemonError> {
        let deadline = Instant::now() + self.ctx.settings.timing.shutdown_grace;
        self.ctx.stopping.store(true, Ordering::Relaxed);
        self.stop.send_replace(true);
        let drained = tokio::time::timeout_at(deadline, async {
            while self.tasks.join_next().await.is_some() {}
            (&mut self.server).await
        })
        .await;
        let Ok(served) = drained else {
            tracing::warn!("abandoning work that did not stop in time; skipping the last export");
            self.tasks.abort_all();
            self.server.abort();
            return Ok(());
        };
        let lock = served?;
        let flush = self.ctx.blocking(ThreadPriority::UserInitiated, |ctx| {
            ctx.export_blocking(None)
        });
        if tokio::time::timeout_at(deadline, flush).await.is_err() {
            tracing::warn!("the last export did not finish in time");
        }
        drop(lock);
        tracing::info!("daemon stopped");
        Ok(())
    }
}

/// Opens everything the tasks share.
async fn build(
    settings: Settings,
) -> Result<(Arc<Ctx>, crate::scheduler::Commands<Job>, EventBus), DaemonError> {
    let seen = config_bytes(&settings.config_path);
    let config = load_config(&settings.config_path, &*settings.env)?;
    let ingest = IngestSettings::from_config(&config, &settings.paths, &*settings.env);
    let path = store_path(&settings.paths, &config);
    let open_path = path.clone();
    let open_ingest = ingest.clone();
    let stopping = Arc::new(AtomicBool::new(false));
    let open_stopping = Arc::clone(&stopping);
    let (store, beat, exports, reader, ingestor) = tokio::task::spawn_blocking(move || {
        let store = Store::open(&open_path)?;
        let beat = Store::open(&open_path)?;
        let exports = Store::open(&open_path)?;
        let reader = StoreReader::open(&open_path)?;
        let ingestor = Ingestor::open(open_ingest, &open_path)?.with_stop(open_stopping);
        Ok::<_, DaemonError>((store, beat, exports, reader, ingestor))
    })
    .await??;

    let sinks = Ctx::build_sinks(&settings.paths, &config);
    let plan = watch_plan(&config, &ingest, &ingestor);
    let (scheduler, commands) = Scheduler::new();
    let (watch_plan, _) = watch::channel(plan);
    let bus = EventBus::new();
    let ctx = Arc::new(Ctx {
        settings,
        store_path: path,
        store: Mutex::new(store),
        beat: Mutex::new(beat),
        exports: Mutex::new(exports),
        reader: Mutex::new(reader),
        ingestor: Mutex::new(ingestor),
        stopping,
        sinks: Mutex::new(sinks),
        active: RwLock::new(Arc::new(Active { config })),
        run: Mutex::new(Runtime {
            config_bytes: seen,
            ..Runtime::default()
        }),
        bus: bus.clone(),
        scheduler,
        watch_plan,
        http: OnceLock::new(),
        started_at: Timestamp::now(),
        stop_requested: Notify::new(),
    });
    ctx.blocking(ThreadPriority::UserInitiated, |ctx| {
        // What the store holds now: only later movement is a change worth announcing.
        let heads: HashMap<_, _> = ctx.heads()?;
        // A window that reset while the daemon was down still raises `quota_reset`.
        let resets = stored_quota_resets(&lock(&ctx.reader))?;
        let mut run = lock(&ctx.run);
        run.heads = heads;
        run.quota_resets = resets;
        drop(run);
        ctx.sync_subscriptions(&ctx.active().config);
        Ok::<_, DaemonError>(())
    })
    .await??;
    Ok((ctx, commands, bus))
}

/// The `resets_at` of the newest stored reading of every Claude quota window.
fn stored_quota_resets(reader: &StoreReader) -> Result<QuotaResets, DaemonError> {
    const PAGE: usize = 1_000;
    let mut newest: HashMap<(ProfileId, WindowId), (Timestamp, Option<Timestamp>)> = HashMap::new();
    let mut after = Revision::ZERO;
    loop {
        let page = reader.quota_snapshots_after(after, PAGE)?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.revision;
        let full = page.len() == PAGE;
        for Versioned { row, .. } in page {
            let QuotaReading::Window(window) = row.reading else {
                continue;
            };
            if row.agent != AgentId::CLAUDE {
                continue;
            }
            let entry = newest
                .entry((row.profile, window.id))
                .or_insert((row.fetched_at, window.state.resets_at));
            if row.fetched_at >= entry.0 {
                *entry = (row.fetched_at, window.state.resets_at);
            }
        }
        if !full {
            break;
        }
    }
    Ok(newest
        .into_iter()
        .map(|(key, (_, resets_at))| (key, resets_at))
        .collect())
}

/// The scheduler task: runs due jobs, each as its own task, and waits for them on the way out.
async fn scheduler_task(
    ctx: Arc<Ctx>,
    commands: crate::scheduler::Commands<Job>,
    shutdown: Shutdown,
) {
    let mut jobs: JoinSet<()> = JoinSet::new();
    commands
        .run(shutdown, |job| {
            while jobs.try_join_next().is_some() {}
            jobs.spawn(run_job(Arc::clone(&ctx), job));
        })
        .await;
    while jobs.join_next().await.is_some() {}
}

/// Runs one job and re-arms it if the config still wants it.
async fn run_job(ctx: Arc<Ctx>, job: Job) {
    let timing = ctx.settings.timing;
    match job {
        Job::Heartbeat => {
            let beat = ctx
                .blocking(ThreadPriority::Background, |ctx| {
                    lock(&ctx.beat).set_heartbeat()
                })
                .await;
            match beat {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "cannot write the heartbeat"),
                Err(error) => tracing::warn!(%error, "heartbeat task failed"),
            }
            ctx.schedule_periodic(Job::Heartbeat, timing.heartbeat);
        }
        Job::Sweep => {
            let request = ScanRequest::incremental(ScanPriority::Background);
            if let Err(error) = ctx.scan(request).await {
                tracing::warn!(%error, "periodic scan failed");
            }
            ctx.schedule_periodic(
                Job::Sweep,
                std_duration(ctx.active().config.daemon.scan_interval),
            );
        }
        Job::Backup => {
            let interval = std_duration(ctx.active().config.backup.interval);
            if !ctx.active().config.backup.enabled {
                return;
            }
            let next = match ctx.backup(ThreadPriority::Background).await {
                Ok(_) => interval,
                Err(error) => {
                    tracing::warn!(%error, "backup failed");
                    interval.min(timing.network_retry)
                }
            };
            ctx.schedule_periodic(Job::Backup, next);
        }
        Job::Quota => {
            if !ctx.active().config.quota.enabled {
                return;
            }
            let interval = std_duration(ctx.active().config.quota.interval);
            let next = match ctx.poll_quota().await {
                Ok(_) => interval,
                Err(error) => {
                    tracing::warn!(%error, "quota poll failed");
                    interval
                }
            };
            ctx.schedule_periodic(Job::Quota, next);
        }
        Job::Fx => {
            let config = ctx.active();
            if !config.config.fx.enabled || config.config.fx.fixed_eur_per_usd.is_some() {
                return;
            }
            let next = match ctx.refresh_fx().await {
                Ok(()) => timing.fx_interval,
                Err(error) => {
                    tracing::warn!(%error, "FX refresh failed");
                    timing.network_retry
                }
            };
            ctx.scheduler.schedule_in(Job::Fx, next, ctx.slack(next));
        }
        Job::Pricing => {
            if !ctx.active().config.pricing.auto_update {
                return;
            }
            let next = match ctx.refresh_pricing().await {
                Ok(_) => timing.pricing_interval,
                Err(error) => {
                    tracing::warn!(%error, "price list refresh failed");
                    timing.network_retry
                }
            };
            ctx.scheduler
                .schedule_in(Job::Pricing, next, ctx.slack(next));
        }
        Job::Export(sink) => {
            if !ctx.active().config.sinks.iter().any(|s| s.id == sink) {
                return;
            }
            if let Err(error) = ctx.export(Some(sink), ThreadPriority::Background).await {
                tracing::warn!(%error, "export task failed");
            }
        }
        Job::Publish => ctx.publish(),
        Job::Reload => {
            // The error is logged and kept as `config_error` by `reload`.
            let _ = ctx.reload(ReloadCause::FileEvent).await;
        }
    }
}

#[cfg(test)]
mod tests;
