//! Actions that need the only writer gone: the daemon lock, the stop/restart around a
//! destructive action, and restore.
//!
//! The daemon holds `<data dir>/daemon.lock` for as long as it serves. Whoever writes the store
//! without the daemon takes the same lock first and keeps it for the whole write, so a daemon
//! started meanwhile (by launchd, the app or a terminal) fails to start instead of racing.

use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use camino::Utf8Path;
use tokenmtr_platform::control::DaemonLock;
use tokenmtr_platform::{ControlError, ControlTransport, ServiceManager};

use crate::error::AppError;

/// How long a stopping daemon gets to release the daemon lock.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// Interval between two attempts at the daemon lock.
const LOCK_POLL: Duration = Duration::from_millis(50);

/// Whether a daemon accepts connections on its control socket.
#[must_use]
pub fn daemon_running(transport: &ControlTransport) -> bool {
    UnixStream::connect(transport.socket_path()).is_ok()
}

/// Takes the daemon lock, waiting up to `grace` for a stopping daemon to release it. While the
/// returned lock lives no daemon can start, so the caller is the only writer.
///
/// # Errors
///
/// [`AppError::DaemonRunning`] when a daemon still holds the lock, or (for a daemon older than
/// the lock) still answers on its socket, after `grace`; [`AppError::Io`] when the lock file
/// cannot be opened.
pub fn lock_daemon(transport: &ControlTransport, grace: Duration) -> Result<DaemonLock, AppError> {
    let deadline = Instant::now() + grace;
    loop {
        let waiting = match transport.lock() {
            Ok(lock) if !daemon_running(transport) => return Ok(lock),
            Ok(_) | Err(ControlError::Locked(_) | ControlError::AlreadyRunning(_)) => {
                Instant::now() < deadline
            }
            Err(ControlError::Io(source)) => {
                return Err(AppError::io(transport.lock_path(), source));
            }
        };
        if !waiting {
            return Err(AppError::DaemonRunning {
                socket: transport.socket_path().to_owned(),
            });
        }
        thread::sleep(LOCK_POLL);
    }
}

/// Runs `action` with the daemon stopped: unloads the service, holds the daemon lock
/// for the whole of `action`, then loads the service again if it was loaded, even when `action`
/// failed.
///
/// A daemon outside the service manager (`tokenmtr run` in a terminal) must be asked to shut
/// down first; this waits [`STOP_GRACE`] for it.
///
/// # Errors
///
/// [`AppError::Service`] when the service cannot be stopped or started again,
/// [`AppError::DaemonRunning`] as [`lock_daemon`], or the error of `action`, which wins over a
/// restart failure.
pub fn with_daemon_stopped<T>(
    transport: &ControlTransport,
    service: &dyn ServiceManager,
    action: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    stopped_within(transport, service, STOP_GRACE, action)
}

pub(crate) fn stopped_within<T>(
    transport: &ControlTransport,
    service: &dyn ServiceManager,
    grace: Duration,
    action: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let was_loaded = service.stop()?;
    let outcome = lock_daemon(transport, grace).and_then(|lock| {
        let outcome = action();
        // Released before the restart: the new daemon needs it.
        drop(lock);
        outcome
    });
    if !was_loaded {
        return outcome;
    }
    let restarted = service.start();
    let value = outcome?;
    restarted?;
    Ok(value)
}

/// Replaces the store at `store_path` with the backup `backup` with the daemon stopped
/// ([`with_daemon_stopped`]): decompress next to it, `integrity_check`, refuse a newer schema
///, drop the WAL files, rename into place.
///
/// # Errors
///
/// As [`with_daemon_stopped`]; [`AppError::Store`] when the backup is unreadable, corrupt or
/// newer. The current store is untouched then.
pub fn restore_backup(
    transport: &ControlTransport,
    service: &dyn ServiceManager,
    store_path: &Utf8Path,
    backup: &Utf8Path,
) -> Result<(), AppError> {
    with_daemon_stopped(transport, service, || {
        Ok(tokenmtr_store::restore(backup, store_path)?)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::os::unix::net::UnixListener;

    use camino::Utf8PathBuf;
    use tokenmtr_platform::{ServiceError, ServiceStatus};
    use tokenmtr_store::{Store, StoreRead, StoreReader};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A service manager that records its calls.
    #[derive(Default)]
    pub(crate) struct FakeService {
        pub loaded: Cell<bool>,
        pub calls: RefCell<Vec<&'static str>>,
    }

    impl FakeService {
        pub(crate) fn loaded() -> Self {
            Self {
                loaded: Cell::new(true),
                calls: RefCell::default(),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }
    }

    impl ServiceManager for FakeService {
        fn install(&self, _binary: &Utf8Path) -> Result<(), ServiceError> {
            self.record("install");
            self.loaded.set(true);
            Ok(())
        }

        fn uninstall(&self) -> Result<(), ServiceError> {
            self.record("uninstall");
            self.loaded.set(false);
            Ok(())
        }

        fn stop(&self) -> Result<bool, ServiceError> {
            self.record("stop");
            Ok(self.loaded.replace(false))
        }

        fn start(&self) -> Result<(), ServiceError> {
            self.record("start");
            self.loaded.set(true);
            Ok(())
        }

        fn status(&self) -> Result<ServiceStatus, ServiceError> {
            Ok(if self.loaded.get() {
                ServiceStatus::Loaded
            } else {
                ServiceStatus::NotInstalled
            })
        }
    }

    fn transport(dir: &tempfile::TempDir) -> Result<ControlTransport, &'static str> {
        let path = Utf8PathBuf::from_path_buf(dir.path().join("control.sock"))
            .map_err(|_| "temp dir is not UTF-8")?;
        Ok(ControlTransport::unix(path))
    }

    #[test]
    fn a_listening_socket_means_a_running_daemon() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        assert!(!daemon_running(&transport));
        let _listener = UnixListener::bind(transport.socket_path())?;
        assert!(daemon_running(&transport));
        assert!(matches!(
            lock_daemon(&transport, Duration::ZERO),
            Err(AppError::DaemonRunning { .. })
        ));
        Ok(())
    }

    #[test]
    fn the_lock_excludes_a_second_holder_until_released() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        let held = lock_daemon(&transport, Duration::ZERO)?;
        assert!(matches!(
            lock_daemon(&transport, Duration::from_millis(100)),
            Err(AppError::DaemonRunning { .. })
        ));
        drop(held);
        drop(lock_daemon(&transport, Duration::ZERO)?);
        Ok(())
    }

    #[test]
    fn a_failing_action_still_restarts_the_service() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        let service = FakeService::loaded();
        let result: Result<(), AppError> =
            stopped_within(&transport, &service, Duration::ZERO, || {
                // The lock is held while the action runs.
                assert!(transport.lock().is_err());
                Err(AppError::InvalidRequest("boom".to_owned()))
            });
        assert!(matches!(result, Err(AppError::InvalidRequest(_))));
        assert_eq!(*service.calls.borrow(), ["stop", "start"]);
        assert!(service.loaded.get());
        Ok(())
    }

    #[test]
    fn an_unloaded_service_stays_unloaded() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        let service = FakeService::default();
        stopped_within(&transport, &service, Duration::ZERO, || Ok(()))?;
        assert_eq!(*service.calls.borrow(), ["stop"]);
        Ok(())
    }

    #[test]
    fn restore_refuses_while_the_daemon_runs_and_keeps_the_store() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        let _daemon = transport.lock()?;
        let store_path = Utf8PathBuf::from_path_buf(dir.path().join("usage.db"))
            .map_err(|_| "temp dir is not UTF-8")?;
        drop(Store::open(&store_path)?);
        let before = std::fs::read(&store_path)?;
        let result = stopped_within(&transport, &FakeService::default(), Duration::ZERO, || {
            Ok(tokenmtr_store::restore(
                Utf8Path::new("/nonexistent.zst"),
                &store_path,
            )?)
        });
        assert!(matches!(result, Err(AppError::DaemonRunning { .. })));
        assert_eq!(std::fs::read(&store_path)?, before);
        Ok(())
    }

    #[test]
    fn backup_then_restore_round_trips() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        let store_path = Utf8PathBuf::from_path_buf(dir.path().join("usage.db"))
            .map_err(|_| "temp dir is not UTF-8")?;
        let backup = Utf8PathBuf::from_path_buf(dir.path().join("b.db.zst"))
            .map_err(|_| "temp dir is not UTF-8")?;
        drop(Store::open(&store_path)?);
        StoreReader::open(&store_path)?.backup(&backup)?;
        std::fs::remove_file(&store_path)?;
        let service = FakeService::loaded();
        restore_backup(&transport, &service, &store_path, &backup)?;
        assert_eq!(*service.calls.borrow(), ["stop", "start"]);
        drop(StoreReader::open(&store_path)?);
        Ok(())
    }
}
