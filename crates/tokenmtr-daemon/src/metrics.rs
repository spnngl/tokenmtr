//! Metrics outputs: Prometheus exporters and remote-write pushers, all fed by one snapshot.
//!
//! - `Job::MetricsRefresh` takes a snapshot of the store, folds it once per output (exclusions,
//!   redaction) and publishes the result: the pre-rendered bodies of each exporter, the fold of
//!   each remote-write output. It follows store writes at most once per `refresh_interval`, and
//!   is armed at the earliest session expiry, so time alone retires a session's series.
//! - `Job::ExporterBind(id)` (re)starts one exporter's server, retrying a failed bind with
//!   back-off. A scrape never reads the store.
//! - `Job::RemoteWrite(id)` pushes one output's latest fold every `interval`.
//!
//! Every start, bind and push is a `metrics_run` row; the failing streak feeds the
//! `metrics_failing` alert exactly like a failing sink.

mod secret;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use jiff::Timestamp;
use tokenmtr_app::display_zone;
use tokenmtr_config::{
    Auth, Config, ExporterConfig, ExternalLabels, OutputFilter, RemoteWriteConfig,
};
use tokenmtr_core::SinkId;
use tokenmtr_metrics::text::Rendered;
use tokenmtr_metrics::{Folded, OutputScope, Seen, SnapshotContext, requests, snapshot};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_query::{METRICS_HEALTH_RUNS, MetricsState, metrics_state};
use tokenmtr_remote::remote_write::{Target, push};
use tokenmtr_remote::{Credential, build_push_client, scrape};
use tokenmtr_store::{Dataset, MetricsRun, Store, StoreRead, StoreReader};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::alerts::Evaluation;
use crate::changes::std_duration;
use crate::ctx::{Ctx, Job, lock};
use crate::export::before_alert;

/// How long a replaced server gets to let go of its port before the next one binds.
const REBIND_WAIT: Duration = Duration::from_secs(3);

/// What the exporter's server is bound to: a change of either means a new server.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    listen: std::net::SocketAddr,
    auth: Option<Auth>,
}

impl Binding {
    fn of(config: &ExporterConfig) -> Self {
        Self {
            listen: config.listen,
            auth: config.auth.clone(),
        }
    }
}

/// A running scrape server.
struct Server {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Server {
    /// Asks the server to finish and hands back its task.
    fn signal(self) -> JoinHandle<()> {
        let _ = self.stop.send(());
        self.task
    }

    async fn stop(self) {
        let task = self.signal();
        if tokio::time::timeout(REBIND_WAIT, task).await.is_err() {
            tracing::warn!("a metrics server did not stop in time");
        }
    }
}

struct Exporter {
    bound: Binding,
    /// The filter the published bodies were folded with.
    filter: OutputFilter,
    rendered: watch::Sender<Arc<Rendered>>,
    server: Option<Server>,
    series: u64,
}

/// What a push needs besides the fold; resolved when the output starts (P14).
#[derive(Clone)]
struct Prepared {
    target: Target,
    external: Vec<(String, String)>,
}

struct Writer {
    config: RemoteWriteConfig,
    folded: Option<Arc<Folded>>,
    seen: Seen,
    prepared: Option<Prepared>,
}

#[derive(Default)]
struct State {
    exporters: HashMap<SinkId, Exporter>,
    writers: HashMap<SinkId, Writer>,
    last_refresh: Option<Instant>,
    /// Whether any refresh has published yet.
    refreshed: bool,
    /// Servers of removed exporters, awaited at shutdown.
    retired: Vec<JoinHandle<()>>,
}

/// Everything the metrics jobs share, owned by the [`Ctx`].
pub(crate) struct Metrics {
    /// Writes the run history; its own connection, so a slow export or scan never delays it.
    store: Mutex<Store>,
    /// Reads the snapshot; its own connection, so a refresh never delays a control request.
    reader: Mutex<StoreReader>,
    state: Mutex<State>,
    push_client: OnceLock<reqwest::Client>,
}

impl Metrics {
    pub(crate) fn new(store: Store, reader: StoreReader) -> Self {
        Self {
            store: Mutex::new(store),
            reader: Mutex::new(reader),
            state: Mutex::new(State::default()),
            push_client: OnceLock::new(),
        }
    }
}

/// Whether any output is configured.
fn enabled(config: &Config) -> bool {
    !config.metrics.exporters.is_empty() || !config.metrics.remote_write.is_empty()
}

fn count(series: usize) -> u64 {
    u64::try_from(series).unwrap_or(u64::MAX)
}

/// What a push attempt did.
struct Pushed {
    series: u64,
    error: Option<String>,
    /// Whether trying again soon can help.
    retry: bool,
}

impl Pushed {
    fn failed(error: impl Into<String>, retry: bool) -> Self {
        Self {
            series: 0,
            error: Some(error.into()),
            retry,
        }
    }
}

impl Ctx {
    /// Plans the metrics jobs `new` needs and retires those it drops. State alone decides what
    /// changed, so a reload that leaves an output alone keeps its server, its `Seen` set and its
    /// timers.
    pub(crate) fn arm_metrics(&self, old: Option<&Config>, new: &Config) {
        let outputs = &new.metrics;
        let mut state = lock(&self.metrics.state);

        let gone: Vec<SinkId> = state
            .exporters
            .keys()
            .filter(|id| !outputs.exporters.iter().any(|e| e.id == **id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(exporter) = state.exporters.remove(&id)
                && let Some(server) = exporter.server
            {
                state.retired.push(server.signal());
            }
            self.scheduler.cancel(Job::ExporterBind(id));
        }
        for config in &outputs.exporters {
            let binding = Binding::of(config);
            if let Some(exporter) = state.exporters.get_mut(&config.id)
                && exporter.bound == binding
            {
                exporter.filter = config.filter.clone();
                continue;
            }
            let previous = state.exporters.remove(&config.id);
            // The bodies carry over, so a rebind never serves an empty page.
            let rendered = previous.as_ref().map_or_else(
                || watch::channel(Arc::new(Rendered::default())).0,
                |p| p.rendered.clone(),
            );
            let series = previous.as_ref().map_or(0, |p| p.series);
            if let Some(server) = previous.and_then(|p| p.server) {
                state.retired.push(server.signal());
            }
            state.exporters.insert(
                config.id.clone(),
                Exporter {
                    bound: binding,
                    filter: config.filter.clone(),
                    rendered,
                    server: None,
                    series,
                },
            );
            self.schedule_now(Job::ExporterBind(config.id.clone()));
        }

        state
            .writers
            .retain(|id, _| outputs.remote_write.iter().any(|w| w.id == *id));
        for config in &outputs.remote_write {
            if state
                .writers
                .get(&config.id)
                .is_some_and(|w| w.config == *config)
            {
                continue;
            }
            state.writers.insert(
                config.id.clone(),
                Writer {
                    config: config.clone(),
                    folded: None,
                    seen: Seen::default(),
                    prepared: None,
                },
            );
            self.schedule_now(Job::RemoteWrite(config.id.clone()));
        }
        for id in old
            .into_iter()
            .flat_map(|o| &o.metrics.remote_write)
            .filter(|w| !outputs.remote_write.iter().any(|n| n.id == w.id))
        {
            self.scheduler.cancel(Job::RemoteWrite(id.id.clone()));
        }
        drop(state);

        if !enabled(new) {
            self.scheduler.cancel(Job::MetricsRefresh);
        } else if old
            .is_none_or(|o| o.metrics != new.metrics || o.subscriptions != new.subscriptions)
        {
            self.schedule_now(Job::MetricsRefresh);
        }
    }

    /// Plans a refresh after a store write that changes what the metrics count.
    pub(crate) fn wake_metrics(&self, changed: &BTreeSet<Dataset>, now: Instant) {
        const COUNTED: [Dataset; 4] = [
            Dataset::UsageEvents,
            Dataset::ToolCalls,
            Dataset::Subscriptions,
            Dataset::QuotaSnapshots,
        ];
        let active = self.active();
        if !enabled(&active.config) || !COUNTED.iter().any(|d| changed.contains(d)) {
            return;
        }
        let interval = std_duration(active.config.metrics.refresh_interval);
        let due = lock(&self.metrics.state)
            .last_refresh
            .map_or(now, |at| (at + interval).max(now));
        self.scheduler
            .schedule(Job::MetricsRefresh, due, Duration::ZERO);
    }

    /// Rebuilds the snapshot and publishes it to every output (the `MetricsRefresh` job).
    pub(crate) async fn refresh_metrics(self: &Arc<Self>) {
        let interval = std_duration(self.active().config.metrics.refresh_interval);
        if !enabled(&self.active().config) {
            return;
        }
        lock(&self.metrics.state).last_refresh = Some(Instant::now());
        let taken = self
            .blocking(ThreadPriority::Background, Self::take_snapshot)
            .await;
        match taken {
            Ok(Ok(Some(expiry))) => {
                // Time alone retires a session; wake once its series are due to go.
                let wait = expiry
                    .duration_since(Timestamp::now())
                    .max(jiff::SignedDuration::ZERO);
                self.scheduler.schedule_in(
                    Job::MetricsRefresh,
                    std_duration(wait) + Duration::from_secs(1),
                    Duration::ZERO,
                );
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "cannot take the metrics snapshot");
                self.scheduler
                    .schedule_in(Job::MetricsRefresh, interval, Duration::ZERO);
            }
            Err(error) => tracing::warn!(%error, "metrics snapshot task failed"),
        }
    }

    /// Reads the snapshot, folds it for every output and publishes the results. Returns when
    /// the next session expires.
    fn take_snapshot(&self) -> Result<Option<Timestamp>, crate::error::DaemonError> {
        let active = self.active();
        let config = &active.config;
        let context = SnapshotContext::new(
            display_zone(config),
            config.subscriptions.clone(),
            config.metrics.session_window,
        );
        let taken = {
            let reader = lock(&self.metrics.reader);
            let snapshot = snapshot(&reader, &context, Timestamp::now())?;
            let key = reader.redaction_key();
            let exporters: Vec<_> = config
                .metrics
                .exporters
                .iter()
                .map(|e| {
                    let folded = snapshot.fold(&OutputScope::new(&e.filter, key));
                    (
                        e.id.clone(),
                        e.filter.clone(),
                        Arc::new(Rendered::new(&folded)),
                        count(folded.series.len()),
                    )
                })
                .collect();
            let writers: Vec<_> = config
                .metrics
                .remote_write
                .iter()
                .map(|w| {
                    let folded = snapshot.fold(&OutputScope::new(&w.filter, key));
                    (w.id.clone(), w.filter.clone(), Arc::new(folded))
                })
                .collect();
            (exporters, writers, snapshot.next_expiry())
        };
        let (exporters, writers, expiry) = taken;
        let mut state = lock(&self.metrics.state);
        // A reload since the snapshot started may have changed an output's filter: that output's
        // own refresh follows, so this fold is dropped.
        for (id, filter, rendered, series) in exporters {
            if let Some(exporter) = state.exporters.get_mut(&id)
                && exporter.filter == filter
            {
                exporter.rendered.send_replace(rendered);
                exporter.series = series;
            }
        }
        for (id, filter, folded) in writers {
            if let Some(writer) = state.writers.get_mut(&id)
                && writer.config.filter == filter
            {
                writer.folded = Some(folded);
            }
        }
        state.refreshed = true;
        Ok(expiry)
    }

    /// (Re)starts the server of one exporter (the `ExporterBind` job).
    pub(crate) async fn bind_exporter(self: &Arc<Self>, id: SinkId) {
        let Some(config) = self
            .active()
            .config
            .metrics
            .exporters
            .iter()
            .find(|e| e.id == id)
            .cloned()
        else {
            return;
        };
        // The first scrape must not see an empty page.
        if !lock(&self.metrics.state).refreshed {
            self.refresh_metrics().await;
        }
        let old = lock(&self.metrics.state)
            .exporters
            .get_mut(&id)
            .and_then(|e| e.server.take());
        if let Some(old) = old {
            old.stop().await;
        }
        let started = Timestamp::now();
        match self.start_server(&config).await {
            Ok(server) => {
                // `Err` hands the server back: the exporter was removed meanwhile.
                let installed = {
                    let mut state = lock(&self.metrics.state);
                    match state.exporters.get_mut(&id) {
                        Some(exporter) => {
                            exporter.server = Some(server);
                            Ok(exporter.series)
                        }
                        None => Err(server),
                    }
                };
                let series = match installed {
                    Ok(series) => series,
                    Err(server) => {
                        server.stop().await;
                        return;
                    }
                };
                tracing::info!(exporter = %id, listen = %config.listen, "metrics exporter listening");
                self.note_metrics(&id, started, series, None);
            }
            Err(error) => {
                tracing::warn!(exporter = %id, %error, "metrics exporter cannot start");
                let (since, failures) = self.note_metrics(&id, started, 0, Some(&error));
                let delay = self
                    .backoff(failures)
                    .min(self.settings.timing.export_retry_cap);
                let delay = before_alert(delay, since, &self.active().config);
                self.scheduler
                    .schedule_in(Job::ExporterBind(id), delay, Duration::ZERO);
            }
        }
    }

    async fn start_server(self: &Arc<Self>, config: &ExporterConfig) -> Result<Server, String> {
        let auth = match config.auth.clone() {
            Some(auth) => {
                let resolved = self
                    .blocking(ThreadPriority::Background, move |ctx| {
                        secret::credential(&*ctx.settings.env, &auth)
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                Some(resolved?)
            }
            None => None,
        };
        let listener = TcpListener::bind(config.listen)
            .await
            .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
        if auth.is_none() && !config.listen.ip().is_loopback() {
            tracing::warn!(
                exporter = %config.id,
                listen = %config.listen,
                "the metrics exporter is open to the network without authentication"
            );
        }
        let receiver = lock(&self.metrics.state)
            .exporters
            .get(&config.id)
            .map(|e| e.rendered.subscribe())
            .ok_or("the exporter was removed")?;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let shutdown = async {
                let _ = stopped.await;
            };
            if let Err(error) = scrape::serve(listener, receiver, auth.as_ref(), shutdown).await {
                tracing::warn!(%error, "the metrics server ended");
            }
        });
        Ok(Server { stop, task })
    }

    /// Pushes one remote-write output's latest fold (the `RemoteWrite` job).
    pub(crate) async fn push_remote_write(self: &Arc<Self>, id: SinkId) {
        let Some(config) = self
            .active()
            .config
            .metrics
            .remote_write
            .iter()
            .find(|w| w.id == id)
            .cloned()
        else {
            return;
        };
        let started = Timestamp::now();
        let pushed = self.push_once(&config).await;
        let interval = std_duration(config.interval);
        let (since, failures) =
            self.note_metrics(&id, started, pushed.series, pushed.error.as_deref());
        let delay = if pushed.error.is_some() && pushed.retry {
            self.backoff(failures).min(interval)
        } else {
            interval
        };
        let delay = if pushed.error.is_some() {
            before_alert(delay, since, &self.active().config)
        } else {
            delay
        };
        self.scheduler
            .schedule_in(Job::RemoteWrite(id), delay, self.slack(delay));
    }

    async fn push_once(self: &Arc<Self>, config: &RemoteWriteConfig) -> Pushed {
        let prepared = match self.prepared_writer(config).await {
            Ok(prepared) => prepared,
            Err(error) => return Pushed::failed(error, true),
        };
        let mut folded = self.latest_fold(&config.id);
        if folded.is_none() {
            self.refresh_metrics().await;
            folded = self.latest_fold(&config.id);
        }
        let Some(folded) = folded else {
            return Pushed::failed("no snapshot yet", true);
        };
        let client = match self.push_client() {
            Ok(client) => client,
            Err(error) => return Pushed::failed(error, true),
        };
        let now = Timestamp::now();
        let window = self.active().config.metrics.session_window;
        let batches = match lock(&self.metrics.state).writers.get(&config.id) {
            Some(writer) => requests(&folded, &writer.seen, &prepared.external, now, window),
            None => return Pushed::failed("the output was removed", false),
        };
        let mut pushed = Pushed {
            series: 0,
            error: None,
            retry: false,
        };
        for batch in &batches {
            let body = match batch.encode() {
                Ok(body) => body,
                Err(error) => {
                    pushed.error.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
            match push(client, &prepared.target, &body).await {
                Ok(()) => {
                    pushed.series += count(batch.series());
                    if let Some(writer) = lock(&self.metrics.state).writers.get_mut(&config.id) {
                        writer.seen.accept(batch, now, window);
                    }
                }
                Err(error) => {
                    tracing::warn!(output = %config.id, %error, "remote write failed");
                    pushed.error.get_or_insert_with(|| error.to_string());
                    if error.is_retryable() {
                        // The receiver is down or throttling: the rest would fail the same way.
                        pushed.retry = true;
                        break;
                    }
                }
            }
        }
        pushed
    }

    fn latest_fold(&self, id: &SinkId) -> Option<Arc<Folded>> {
        lock(&self.metrics.state)
            .writers
            .get(id)
            .and_then(|w| w.folded.clone())
    }

    /// The target and labels of `config`, resolved on the first push after the output started.
    async fn prepared_writer(
        self: &Arc<Self>,
        config: &RemoteWriteConfig,
    ) -> Result<Prepared, String> {
        if let Some(prepared) = lock(&self.metrics.state)
            .writers
            .get(&config.id)
            .and_then(|w| w.prepared.clone())
        {
            return Ok(prepared);
        }
        let resolved = config.clone();
        let prepared = self
            .blocking(ThreadPriority::Background, move |ctx| {
                prepare(ctx, &resolved)
            })
            .await
            .map_err(|e| e.to_string())??;
        if let Some(writer) = lock(&self.metrics.state).writers.get_mut(&config.id)
            && writer.config == *config
        {
            writer.prepared = Some(prepared.clone());
        }
        Ok(prepared)
    }

    fn push_client(&self) -> Result<&reqwest::Client, String> {
        if let Some(client) = self.metrics.push_client.get() {
            return Ok(client);
        }
        let client = build_push_client().map_err(|e| e.to_string())?;
        Ok(self.metrics.push_client.get_or_init(|| client))
    }

    /// Records one attempt of `output`, updates its failing streak and evaluates the alerts.
    /// Returns when the current streak began and how many attempts in a row failed.
    fn note_metrics(
        &self,
        output: &SinkId,
        started: Timestamp,
        series: u64,
        error: Option<&str>,
    ) -> (Timestamp, u32) {
        let finished = Timestamp::now();
        let run = MetricsRun {
            output: output.clone(),
            started,
            finished,
            series,
            error: error.map(str::to_owned),
        };
        if let Err(error) = lock(&self.metrics.store).record_metrics_run(&run) {
            tracing::warn!(%error, "cannot record a metrics run");
        }
        let known = lock(&self.run)
            .metrics
            .get(output)
            .and_then(|runtime| runtime.failing_since);
        let since = match (error, known) {
            (None, _) => finished,
            (Some(_), Some(since)) => since,
            (Some(_), None) => self.metrics_failing_since(output, finished),
        };
        let failures = {
            let mut run = lock(&self.run);
            let runtime = run.metrics.entry(output.clone()).or_default();
            runtime.last_attempt = Some(Instant::now());
            match error {
                None => {
                    runtime.failures = 0;
                    runtime.last_ok_at = Some(finished);
                    runtime.last_error = None;
                    runtime.failing_since = None;
                }
                Some(error) => {
                    runtime.failures = runtime.failures.saturating_add(1);
                    runtime.last_error = Some(error.to_owned());
                    runtime.failing_since = Some(since);
                }
            }
            runtime.failures
        };
        self.evaluate_alerts(Evaluation::default());
        (since, failures)
    }

    /// Start of the failing streak of `output` according to its run history; `now` when the
    /// history shows none.
    fn metrics_failing_since(&self, output: &SinkId, now: Timestamp) -> Timestamp {
        let runs = lock(&self.reader)
            .metrics_runs(output, METRICS_HEALTH_RUNS)
            .unwrap_or_default();
        match metrics_state(&runs) {
            MetricsState::Failing { since, .. } => since,
            MetricsState::Healthy => now,
        }
    }

    /// The wait after `failures` failed attempts in a row: `metrics_backoff` doubled each time.
    fn backoff(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        self.settings
            .timing
            .metrics_backoff
            .saturating_mul(1_u32 << doublings)
    }

    /// Stops every exporter's server and waits for them (shutdown).
    pub(crate) async fn stop_metrics(&self) {
        let (servers, retired) = {
            let mut state = lock(&self.metrics.state);
            let servers: Vec<Server> = state
                .exporters
                .values_mut()
                .filter_map(|e| e.server.take())
                .collect();
            (servers, std::mem::take(&mut state.retired))
        };
        let mut tasks: Vec<JoinHandle<()>> = servers.into_iter().map(Server::signal).collect();
        tasks.extend(retired);
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// The target and the labels of a remote-write output; blocking (a file, the host name).
fn prepare(ctx: &Ctx, config: &RemoteWriteConfig) -> Result<Prepared, String> {
    let credential: Option<Credential> = config
        .auth
        .as_ref()
        .map(|auth| secret::credential(&*ctx.settings.env, auth))
        .transpose()?;
    let target = Target::new(config.url.clone(), config.tenant.as_deref(), credential)
        .map_err(|e| e.to_string())?;
    let external = match &config.external_labels {
        ExternalLabels::Machine => {
            vec![("machine".to_owned(), machine_name(&ctx.active().config))]
        }
        ExternalLabels::Explicit(labels) => {
            labels.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
    };
    Ok(Prepared { target, external })
}

/// The machine name: the configured one, else the host name.
fn machine_name(config: &Config) -> String {
    config
        .machine
        .name
        .clone()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}
