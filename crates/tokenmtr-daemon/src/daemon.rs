//! Starting and stopping the daemon, and the job runner.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use jiff::Timestamp;
use tokenmtr_agents::ingest::{IngestSettings, Ingestor, ScanPriority, ScanRequest};
use tokenmtr_control::{EventBus, Server, ServerInfo, Shutdown};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_store::{SCHEMA_VERSION, Store, StoreReader};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::changes::std_duration;
use crate::ctx::{Active, Ctx, Job, Runtime, lock};
use crate::error::DaemonError;
use crate::handler::ControlHandler;
use crate::reload::watch_plan;
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
}

impl Daemon {
    /// Starts the daemon: loads the config, opens the store, binds the control socket (`0600`)
    /// and spawns every task. Returns once the socket accepts connections.
    ///
    /// # Errors
    /// [`DaemonError::AlreadyRunning`] when another daemon owns the socket,
    /// [`DaemonError::NewerSchema`] when the store was written by a newer `TokenMtr` (it is left
    /// untouched), [`DaemonError::Config`] when the config is invalid; store and I/O failures
    /// otherwise.
    pub async fn start(settings: Settings) -> Result<Self, DaemonError> {
        // Bind first: the socket is the single-instance lock (D1), so a second daemon fails
        // before it touches the store.
        let listener = settings.transport.bind().await?;
        let socket = settings.transport.socket_path().to_owned();
        let built = build(settings).await;
        let (ctx, commands, server_bus) = match built {
            Ok(parts) => parts,
            Err(error) => {
                drop(listener);
                // Best effort: the socket is ours, nobody else can use it now.
                let _ = std::fs::remove_file(&socket);
                return Err(error);
            }
        };

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
        tasks.spawn(server.serve(shutdown));

        ctx.schedule_now(Job::Heartbeat);
        let active = ctx.active();
        ctx.rearm(None, &active);
        tracing::info!(socket = %socket, "daemon started");
        Ok(Self { ctx, stop, tasks })
    }

    /// Stops the daemon: no new work starts, the running transaction finishes, sinks get a last
    /// export, the socket is removed. Takes at most [`Timing::shutdown_grace`] to wait for
    /// running work; what does not finish by then is abandoned.
    ///
    /// # Errors
    /// Never fails today; reserved for a flush that cannot be abandoned.
    ///
    /// [`Timing::shutdown_grace`]: crate::Timing::shutdown_grace
    #[allow(
        clippy::unused_async,
        reason = "reserved for failures; the signature is public"
    )]
    pub async fn shutdown(mut self) -> Result<(), DaemonError> {
        let deadline = Instant::now() + self.ctx.settings.timing.shutdown_grace;
        self.stop.send_replace(true);
        let drained = tokio::time::timeout_at(deadline, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            tracing::warn!("abandoning work that did not stop in time");
            self.tasks.abort_all();
        }
        let flush = self.ctx.blocking(ThreadPriority::UserInitiated, |ctx| {
            ctx.export_blocking(None)
        });
        if tokio::time::timeout_at(deadline, flush).await.is_err() {
            tracing::warn!("the final export did not finish in time");
        }
        let socket = self.ctx.settings.transport.socket_path();
        if let Err(error) = std::fs::remove_file(socket) {
            tracing::debug!(%error, "cannot remove the control socket");
        }
        tracing::info!("daemon stopped");
        Ok(())
    }
}

/// Opens everything the tasks share.
async fn build(
    settings: Settings,
) -> Result<(Arc<Ctx>, crate::scheduler::Commands<Job>, EventBus), DaemonError> {
    let config = load_config(&settings.config_path, &*settings.env)?;
    let ingest = IngestSettings::from_config(&config, &settings.paths, &*settings.env);
    let path = store_path(&settings.paths, &config);
    let open_path = path.clone();
    let open_ingest = ingest.clone();
    let (store, reader, ingestor) = tokio::task::spawn_blocking(move || {
        let store = Store::open(&open_path)?;
        let reader = StoreReader::open(&open_path)?;
        let ingestor = Ingestor::open(open_ingest, &open_path)?;
        Ok::<_, DaemonError>((store, reader, ingestor))
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
        reader: Mutex::new(reader),
        ingestor: Mutex::new(ingestor),
        sinks: Mutex::new(sinks),
        active: RwLock::new(Arc::new(Active { config, ingest })),
        run: Mutex::new(Runtime::default()),
        bus: bus.clone(),
        scheduler,
        watch_plan,
        http: OnceLock::new(),
        started_at: Timestamp::now(),
    });
    // What the store holds now: only later movement is a change worth announcing.
    let heads: HashMap<_, _> = ctx.heads()?;
    lock(&ctx.run).heads = heads;
    ctx.sync_subscriptions(&ctx.active().config);
    Ok((ctx, commands, bus))
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
                    lock(&ctx.store).set_heartbeat()
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
            let _ = ctx.reload().await;
        }
    }
}
