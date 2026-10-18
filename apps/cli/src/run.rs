//! `tokenmtr run`: the daemon process.

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use camino::Utf8Path;
use tokenmtr_config::{LogFormat, LogLevel};
use tokenmtr_daemon::{Daemon, DaemonError, Settings};
use tokenmtr_platform::{Paths, Platform, PlatformKind, ThreadPriority, set_thread_priority};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Worker threads of the daemon runtime: the work is I/O waits and short transactions (D47).
const WORKER_THREADS: usize = 2;
/// How long the runtime waits for blocking threads after the daemon stopped. With
/// `Timing::shutdown_grace` (1.2 s) a stop stays under 2 s.
const RUNTIME_GRACE: Duration = Duration::from_millis(500);
/// Daily log files kept in the log directory.
const LOG_FILES_KEPT: usize = 14;

#[cfg(target_os = "macos")]
fn secrets() -> Arc<dyn tokenmtr_platform::SecretStore + Send + Sync> {
    Arc::new(tokenmtr_platform::KeychainSecrets)
}

/// Test hook for `tests/e2e`, compiled into debug builds only: release binaries ignore the
/// variable.
#[cfg(debug_assertions)]
mod test_endpoint {
    use std::sync::Arc;

    use anyhow::{Context, ensure};
    use secrecy::SecretString;
    use tokenmtr_daemon::Settings;
    use tokenmtr_platform::{SecretError, SecretStore};
    use tokenmtr_remote::Endpoints;

    /// A loopback base URL that replaces the three production hosts. While it is set the
    /// Keychain is not read either, so a test never touches the real one. Anything but an `http`
    /// URL on a loopback host is refused for every endpoint, so a credential can never be sent
    /// off the machine through this variable.
    const TEST_ENDPOINT_BASE: &str = "TOKENMTR_TEST_ENDPOINT_BASE";

    /// A secret store that holds nothing: credentials then come from files only.
    struct NoSecrets;

    impl SecretStore for NoSecrets {
        fn get(&self, _service: &str) -> Result<Option<SecretString>, SecretError> {
            Ok(None)
        }
    }

    /// Applies [`TEST_ENDPOINT_BASE`] to `settings` when it is set.
    pub(super) fn apply(settings: &mut Settings) -> anyhow::Result<()> {
        let Some(base) = std::env::var_os(TEST_ENDPOINT_BASE) else {
            return Ok(());
        };
        let base = base
            .to_str()
            .with_context(|| format!("{TEST_ENDPOINT_BASE} is not UTF-8"))?;
        let endpoints = Endpoints::at(base)
            .with_context(|| format!("{TEST_ENDPOINT_BASE} is not a valid URL"))?;
        ensure!(
            [&endpoints.anthropic, &endpoints.ecb, &endpoints.github_raw]
                .into_iter()
                .all(|url| url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
            "{TEST_ENDPOINT_BASE} must be an http URL on a loopback host"
        );
        settings.endpoints = Some(endpoints);
        settings.secrets = Arc::new(NoSecrets);
        Ok(())
    }
}

/// Runs the daemon until SIGTERM, SIGINT or a control `shutdown` request.
pub fn run(config: Option<&Utf8Path>) -> anyhow::Result<()> {
    let platform = PlatformKind::detect().context("cannot resolve the platform directories")?;
    let paths = platform.paths().clone();
    let config_path = config.map_or_else(|| paths.config_file().to_owned(), Utf8Path::to_owned);
    let _log_guard = init_tracing(&paths, &config_path)?;
    // launchd appends the daemon's stdout and stderr forever; keep those files bounded.
    if let Err(error) = tokenmtr_platform::service::trim_launchd_logs(paths.logs_dir()) {
        tracing::warn!(%error, "cannot trim the launchd logs");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .enable_all()
        .on_thread_start(|| {
            // Best effort: a failure only means the threads keep the default QoS.
            let _ = set_thread_priority(ThreadPriority::Background);
        })
        .build()
        .context("cannot start the async runtime")?;
    #[cfg_attr(
        not(debug_assertions),
        expect(unused_mut, reason = "the test hook mutates")
    )]
    let mut settings = Settings::new(paths, config_path, platform.control_transport(), secrets());
    #[cfg(debug_assertions)]
    test_endpoint::apply(&mut settings)?;
    let outcome = runtime.block_on(serve(settings));
    runtime.shutdown_timeout(RUNTIME_GRACE);
    outcome
}

/// Serves until a signal or a `shutdown` request, then stops; both exit 0, so launchd
/// (`KeepAlive.SuccessfulExit = false`) does not restart a daemon that was asked to stop.
/// Another daemon already serving also exits 0: there is nothing to restart.
async fn serve(settings: Settings) -> anyhow::Result<()> {
    // Installed before the slow start (migrations), so SIGTERM never kills mid-write.
    let mut signals = Signals::install()?;
    let start = Daemon::start(settings);
    let daemon = tokio::select! {
        started = start => match started {
            Ok(daemon) => daemon,
            Err(error @ DaemonError::AlreadyRunning(_)) => {
                tracing::info!(%error, "not starting");
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        },
        () = signals.next() => {
            tracing::info!("stopped while starting");
            return Ok(());
        }
    };
    tokio::select! {
        () = signals.next() => {}
        () = daemon.stop_requested() => {}
    }
    tracing::info!("stopping");
    daemon.shutdown().await?;
    Ok(())
}

/// SIGTERM and SIGINT, listened to from installation on.
struct Signals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

impl Signals {
    fn install() -> anyhow::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            term: signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?,
            int: signal(SignalKind::interrupt()).context("cannot listen for SIGINT")?,
        })
    }

    async fn next(&mut self) {
        tokio::select! {
            _ = self.term.recv() => {}
            _ = self.int.recv() => {}
        }
    }
}

/// A log layer writing `format` lines to `writer`.
fn fmt_layer<S, W>(format: LogFormat, writer: W, ansi: bool) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let layer = tracing_subscriber::fmt::layer().with_writer(writer);
    match format {
        LogFormat::Text => layer.with_ansi(ansi).boxed(),
        LogFormat::Json => layer.json().boxed(),
    }
}

/// Logs to a daily rolling file in the platform log directory (and to stderr on a terminal).
/// `RUST_LOG` wins over the configured level. The returned guard flushes the file on drop.
fn init_tracing(paths: &Paths, config_path: &Utf8Path) -> anyhow::Result<WorkerGuard> {
    // The daemon reports an invalid config itself; here only the log settings matter.
    let (level, format) = tokenmtr_config::load(config_path)
        .map(|config| (config.daemon.log_level, config.logging.format))
        .unwrap_or_default();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level_directive(level)));
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("tokenmtr")
        .filename_suffix("log")
        .max_log_files(LOG_FILES_KEPT)
        .build(paths.logs_dir())
        .context("cannot open the log directory")?;
    let (file, guard) = tracing_appender::non_blocking(appender);
    let to_file = fmt_layer(format, file, false);
    let to_terminal = std::io::stderr()
        .is_terminal()
        .then(|| fmt_layer(format, std::io::stderr, true));
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

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::Registry;

    use super::*;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn logged(format: LogFormat) -> String {
        let capture = Capture::default();
        let writer = capture.clone();
        let layer = fmt_layer::<Registry, _>(format, move || writer.clone(), false);
        tracing::subscriber::with_default(Registry::default().with(layer), || {
            tracing::info!(rows = 3, "scan finished");
            tracing::warn!("second line");
        });
        let bytes = capture.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn json_format_writes_one_parsable_object_per_line() {
        let text = logged(LogFormat::Json);
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["level"], "INFO");
        assert_eq!(lines[0]["fields"]["message"], "scan finished");
        assert_eq!(lines[0]["fields"]["rows"], 3);
        assert_eq!(lines[1]["level"], "WARN");
    }

    #[test]
    fn text_format_is_not_json() {
        let text = logged(LogFormat::Text);
        assert!(text.contains("scan finished") && text.contains("rows=3"));
        assert!(serde_json::from_str::<serde_json::Value>(text.lines().next().unwrap()).is_err());
    }
}
