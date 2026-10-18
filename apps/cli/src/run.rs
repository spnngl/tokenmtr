//! `tokenmtr run`: the daemon process.

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use camino::Utf8PathBuf;
use clap::Args;
use tokenmtr_config::LogLevel;
use tokenmtr_daemon::{Daemon, Settings};
use tokenmtr_platform::{Paths, Platform, PlatformKind, ThreadPriority, set_thread_priority};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Worker threads of the daemon runtime: the work is I/O waits and short transactions (D47).
const WORKER_THREADS: usize = 2;
/// How long the runtime waits for blocking threads after the daemon stopped.
const RUNTIME_GRACE: Duration = Duration::from_secs(1);

/// Arguments of `tokenmtr run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Config file; defaults to the platform config location.
    #[arg(long)]
    config: Option<Utf8PathBuf>,
}

#[cfg(target_os = "macos")]
fn secrets() -> Arc<dyn tokenmtr_platform::SecretStore + Send + Sync> {
    Arc::new(tokenmtr_platform::KeychainSecrets)
}

/// Runs the daemon until SIGTERM or SIGINT.
pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    let platform = PlatformKind::detect().context("cannot resolve the platform directories")?;
    let paths = platform.paths().clone();
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| paths.config_file().to_owned());
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
    let settings = Settings::new(paths, config_path, platform.control_transport(), secrets());
    let outcome = runtime.block_on(serve(settings));
    runtime.shutdown_timeout(RUNTIME_GRACE);
    outcome
}

async fn serve(settings: Settings) -> anyhow::Result<()> {
    let daemon = Daemon::start(settings).await?;
    wait_for_signal().await?;
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
fn init_tracing(paths: &Paths, config_path: &camino::Utf8Path) -> anyhow::Result<WorkerGuard> {
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
