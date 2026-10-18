//! Erase all data (D56).

use std::fs;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use tokenmtr_config::Config;
use tokenmtr_platform::{ControlTransport, Paths, ServiceManager};

use crate::backup::backup_files;
use crate::error::AppError;
use crate::guard::{daemon_running, ensure_daemon_stopped};
use crate::settings::{backup_dir, store_path};

/// How long a daemon gets to release its socket after the service was removed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// What to delete besides the store, its WAL files, the backups and the logs. Checkpoints live
/// in the store (D15), so they go with it. Sink outputs and agent files are never deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EraseScope {
    /// Also delete the config file.
    pub config: bool,
    /// Also remove the background service (stopping the daemon it keeps alive).
    pub service: bool,
}

/// What an erase removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EraseReport {
    /// The files and directories that existed and are gone.
    pub removed: Vec<Utf8PathBuf>,
    /// Whether the service was removed.
    pub service_removed: bool,
}

/// Deletes the store, its `-wal`/`-shm`/`-journal`/`-lock` files, the backups and the logs, and per
/// `scope` the config and the service (D56).
///
/// # Errors
///
/// [`AppError::DaemonRunning`] when a daemon is still answering after the optional service
/// removal (a running daemon would recreate what is deleted); [`AppError::Service`] and
/// [`AppError::Io`] on failures. Files removed before a failure stay removed.
pub fn erase(
    paths: &Paths,
    transport: &ControlTransport,
    config_path: &Utf8Path,
    config: &Config,
    scope: EraseScope,
    service: &dyn ServiceManager,
) -> Result<EraseReport, AppError> {
    let mut report = EraseReport::default();
    if scope.service {
        service.uninstall()?;
        report.service_removed = true;
        wait_until_stopped(transport);
    }
    ensure_daemon_stopped(transport)?;

    let store = store_path(paths, config);
    for suffix in ["", "-wal", "-shm", "-journal", "-lock"] {
        remove_file(&Utf8PathBuf::from(format!("{store}{suffix}")), &mut report)?;
    }
    for backup in backup_files(&backup_dir(paths, config)) {
        remove_file(&backup, &mut report)?;
    }
    remove_dir(paths.logs_dir(), &mut report)?;
    if scope.config {
        remove_file(config_path, &mut report)?;
    }
    Ok(report)
}

fn wait_until_stopped(transport: &ControlTransport) {
    let deadline = Instant::now() + STOP_GRACE;
    while daemon_running(transport) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

fn remove_file(path: &Utf8Path, report: &mut EraseReport) -> Result<(), AppError> {
    match fs::remove_file(path) {
        Ok(()) => report.removed.push(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(AppError::io(path, error)),
    }
    Ok(())
}

fn remove_dir(path: &Utf8Path, report: &mut EraseReport) -> Result<(), AppError> {
    match fs::remove_dir_all(path) {
        Ok(()) => report.removed.push(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(AppError::io(path, error)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::unix::net::UnixListener;

    use tokenmtr_config::{ConfigFormat, TomlCodec};
    use tokenmtr_platform::{ServiceError, ServiceStatus};
    use tokenmtr_store::Store;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct FakeService {
        uninstalled: Cell<bool>,
    }

    impl ServiceManager for FakeService {
        fn install(&self, _binary: &Utf8Path) -> Result<(), ServiceError> {
            Ok(())
        }

        fn uninstall(&self) -> Result<(), ServiceError> {
            self.uninstalled.set(true);
            Ok(())
        }

        fn status(&self) -> Result<ServiceStatus, ServiceError> {
            Ok(ServiceStatus::NotInstalled)
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: Paths,
        transport: ControlTransport,
        config_path: Utf8PathBuf,
        config: Config,
        sink_output: Utf8PathBuf,
        agent_file: Utf8PathBuf,
    }

    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .map_err(|_| "temp dir is not UTF-8")?;
        let paths = Paths::from_lookup(|key| (key == "HOME").then(|| home.to_string()))?;
        let config =
            tokenmtr_config::load_str(
                "",
                ConfigFormat::Toml(TomlCodec),
                &|_: &str| None::<String>,
            )?;
        let config_path = paths.config_file().to_owned();
        let store = store_path(&paths, &config);
        drop(Store::open(&store)?);
        let backups = backup_dir(&paths, &config);
        fs::create_dir_all(&backups)?;
        fs::write(backups.join("usage-20260101T000000Z.db.zst"), b"x")?;
        fs::write(backups.join("keep-me.txt"), b"x")?;
        fs::create_dir_all(paths.logs_dir())?;
        fs::write(paths.logs_dir().join("daemon.log"), b"x")?;
        fs::create_dir_all(paths.exports_dir())?;
        let sink_output = paths.exports_dir().join("usage_events.csv");
        fs::write(&sink_output, b"x")?;
        fs::create_dir_all(config_path.parent().ok_or("no config dir")?)?;
        fs::write(&config_path, b"")?;
        let agent_file = home.join(".claude/projects/p/s.jsonl");
        fs::create_dir_all(agent_file.parent().ok_or("no parent")?)?;
        fs::write(&agent_file, b"{}\n")?;
        let transport = ControlTransport::unix(home.join("control.sock"));
        Ok(Fixture {
            _dir: dir,
            paths,
            transport,
            config_path,
            config,
            sink_output,
            agent_file,
        })
    }

    fn run(f: &Fixture, scope: EraseScope, service: &FakeService) -> Result<EraseReport, AppError> {
        erase(
            &f.paths,
            &f.transport,
            &f.config_path,
            &f.config,
            scope,
            service,
        )
    }

    #[test]
    fn erase_deletes_the_store_backups_and_logs_but_not_outputs_or_agent_files() -> TestResult {
        let f = fixture()?;
        let report = run(&f, EraseScope::default(), &FakeService::default())?;
        let store = store_path(&f.paths, &f.config);
        assert!(!store.exists());
        assert!(!f.paths.logs_dir().exists());
        let backups = backup_dir(&f.paths, &f.config);
        assert!(!backups.join("usage-20260101T000000Z.db.zst").exists());
        assert!(backups.join("keep-me.txt").exists());
        assert!(f.sink_output.exists());
        assert!(f.agent_file.exists());
        assert!(f.config_path.exists());
        assert!(!report.service_removed);
        assert!(report.removed.contains(&store));
        Ok(())
    }

    #[test]
    fn scope_adds_the_config_and_the_service() -> TestResult {
        let f = fixture()?;
        let service = FakeService::default();
        let report = run(
            &f,
            EraseScope {
                config: true,
                service: true,
            },
            &service,
        )?;
        assert!(!f.config_path.exists());
        assert!(service.uninstalled.get());
        assert!(report.service_removed);
        Ok(())
    }

    #[test]
    fn erase_refuses_while_the_daemon_runs() -> TestResult {
        let f = fixture()?;
        let _listener = UnixListener::bind(f.transport.socket_path())?;
        let result = run(&f, EraseScope::default(), &FakeService::default());
        assert!(matches!(result, Err(AppError::DaemonRunning { .. })));
        assert!(store_path(&f.paths, &f.config).exists());
        Ok(())
    }
}
