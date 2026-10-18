//! `tokenmtr run`: the daemon process.

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use camino::Utf8Path;
use secrecy::SecretString;
use tokenmtr_config::LogLevel;
use tokenmtr_daemon::{Daemon, Settings};
use tokenmtr_platform::{
    Paths, Platform, PlatformKind, SecretError, SecretStore, ThreadPriority, set_thread_priority,
};
use tokenmtr_remote::Endpoints;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Worker threads of the daemon runtime: the work is I/O waits and short transactions (D47).
const WORKER_THREADS: usize = 2;
/// How long the runtime waits for blocking threads after the daemon stopped.
const RUNTIME_GRACE: Duration = Duration::from_secs(1);

#[cfg(target_os = "macos")]
fn secrets() -> Arc<dyn tokenmtr_platform::SecretStore + Send + Sync> {
    Arc::new(tokenmtr_platform::KeychainSecrets)
}

/// Test hook (`tests/e2e`): a loopback base URL that replaces the three production hosts. While it
/// is set the Keychain is not read either, so a test never touches the real one. Anything but an
/// `http` URL on a loopback host is refused, so a credential can never be sent off the machine
/// through this variable.
const TEST_ENDPOINT_BASE: &str = "TOKENMTR_TEST_ENDPOINT_BASE";

/// A secret store that holds nothing: credentials then come from files only.
struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _service: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
}

/// Applies [`TEST_ENDPOINT_BASE`] to `settings` when it is set.
fn apply_test_endpoint(settings: &mut Settings) -> anyhow::Result<()> {
    let Some(base) = std::env::var_os(TEST_ENDPOINT_BASE) else {
        return Ok(());
    };
    let base = base
        .to_str()
        .with_context(|| format!("{TEST_ENDPOINT_BASE} is not UTF-8"))?;
    let endpoints =
        Endpoints::at(base).with_context(|| format!("{TEST_ENDPOINT_BASE} is not a valid URL"))?;
    ensure!(
        endpoints.anthropic.scheme() == "http"
            && matches!(
                endpoints.anthropic.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]")
            ),
        "{TEST_ENDPOINT_BASE} must be an http URL on a loopback host"
    );
    settings.endpoints = Some(endpoints);
    settings.secrets = Arc::new(NoSecrets);
    Ok(())
}

/// Runs the daemon until SIGTERM, SIGINT or a control `shutdown` request.
pub fn run(config: Option<&Utf8Path>) -> anyhow::Result<()> {
    let platform = PlatformKind::detect().context("cannot resolve the platform directories")?;
    let paths = platform.paths().clone();
    let config_path = config.map_or_else(|| paths.config_file().to_owned(), Utf8Path::to_owned);
    let _log_guard = init_tracing(&paths, &config_path)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .enable_all()
        .on_thread_start(|| {
            // Best effort: a failure only means the threads keep the default QoS.
            let _ = set_thread_priority(ThreadPriority::Background);
        })
        .build()
        .context("cannot start the async runtime")?;
    let mut settings = Settings::new(paths, config_path, platform.control_transport(), secrets());
    apply_test_endpoint(&mut settings)?;
    let outcome = runtime.block_on(serve(settings));
    runtime.shutdown_timeout(RUNTIME_GRACE);
    outcome
}

async fn serve(settings: Settings) -> anyhow::Result<()> {
    let daemon = Daemon::start(settings).await?;
    tokio::select! {
        signal = wait_for_signal() => signal?,
        () = daemon.stop_requested() => {}
    }
    tracing::info!("stopping");
    daemon.shutdown().await?;
    Ok(())
}

async fn wait_for_signal() -> anyhow::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    let mut int = signal(SignalKind::interrupt()).context("cannot listen for SIGINT")?;
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    Ok(())
}

/// Logs to a daily rolling file in the platform log directory (and to stderr on a terminal).
/// `RUST_LOG` wins over the configured level. The returned guard flushes the file on drop.
fn init_tracing(paths: &Paths, config_path: &Utf8Path) -> anyhow::Result<WorkerGuard> {
    // The daemon reports an invalid config itself; here only the level matters.
    let level = tokenmtr_config::load(config_path)
        .map(|config| config.daemon.log_level)
        .unwrap_or_default();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level_directive(level)));
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("tokenmtr")
        .filename_suffix("log")
        .build(paths.logs_dir())
        .context("cannot open the log directory")?;
    let (file, guard) = tracing_appender::non_blocking(appender);
    let to_file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file);
    let to_terminal = std::io::stderr()
        .is_terminal()
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    tracing_subscriber::registry()
        .with(filter)
        .with(to_file)
        .with(to_terminal)
        .try_init()
        .context("cannot install the log subscriber")?;
    Ok(guard)
}

fn level_directive(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    }
}
