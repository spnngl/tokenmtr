//! Erase all data.

use std::fs;
use std::io;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use tokenmtr_config::Config;
use tokenmtr_platform::{ControlTransport, Paths, ServiceManager};

use crate::backup::backup_files;
use crate::error::AppError;
use crate::guard::{STOP_GRACE, lock_daemon, stopped_within};
use crate::settings::{backup_dir_in, store_path_in};

/// What to delete besides the store, its WAL files, the backups and the logs. Checkpoints live
/// in the store, so they go with it. Sink outputs and agent files are never deleted.
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
/// `scope` the config and the service.
///
/// The daemon is stopped first and cannot start while files are deleted (see
/// [`with_daemon_stopped`](crate::with_daemon_stopped)); a service that is kept is started again
/// afterwards, on an empty store. `config` is `None` when the config file does not load: an
/// invalid config never blocks erasing, the default locations are erased then.
///
/// # Errors
///
/// [`AppError::DaemonRunning`] when a daemon outside the service manager still runs (a running
/// daemon would recreate what is deleted); [`AppError::Service`] and [`AppError::Io`] on
/// failures. Files removed before a failure stay removed.
pub fn erase(
    paths: &Paths,
    transport: &ControlTransport,
    config_path: &Utf8Path,
    config: Option<&Config>,
    scope: EraseScope,
    service: &dyn ServiceManager,
) -> Result<EraseReport, AppError> {
    erase_within(
        paths,
        transport,
        config_path,
        config,
        scope,
        service,
        STOP_GRACE,
    )
}

fn erase_within(
    paths: &Paths,
    transport: &ControlTransport,
    config_path: &Utf8Path,
    config: Option<&Config>,
    scope: EraseScope,
    service: &dyn ServiceManager,
    grace: Duration,
) -> Result<EraseReport, AppError> {
    let delete = || remove_data(paths, config_path, config, scope.config);
    let removed = if scope.service {
        service.uninstall()?;
        let lock = lock_daemon(transport, grace)?;
        let removed = delete();
        drop(lock);
        removed
    } else {
        stopped_within(transport, service, grace, delete)
    }?;
    Ok(EraseReport {
        removed,
        service_removed: scope.service,
    })
}

fn remove_data(
    paths: &Paths,
    config_path: &Utf8Path,
    config: Option<&Config>,
    with_config: bool,
) -> Result<Vec<Utf8PathBuf>, AppError> {
    let mut removed = Vec::new();
    let store = store_path_in(paths, config);
    for suffix in ["", "-wal", "-shm", "-journal", "-lock"] {
        remove_file(&Utf8PathBuf::from(format!("{store}{suffix}")), &mut removed)?;
    }
    for backup in backup_files(&backup_dir_in(paths, config)) {
        remove_file(&backup, &mut removed)?;
    }
    remove_dir(paths.logs_dir(), &mut removed)?;
    if with_config {
        remove_file(config_path, &mut removed)?;
    }
    Ok(removed)
}

fn remove_file(path: &Utf8Path, removed: &mut Vec<Utf8PathBuf>) -> Result<(), AppError> {
    match fs::remove_file(path) {
        Ok(()) => removed.push(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(AppError::io(path, error)),
    }
    Ok(())
}

fn remove_dir(path: &Utf8Path, removed: &mut Vec<Utf8PathBuf>) -> Result<(), AppError> {
    match fs::remove_dir_all(path) {
        Ok(()) => removed.push(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(AppError::io(path, error)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokenmtr_config::{ConfigFormat, TomlCodec};
    use tokenmtr_store::Store;

    use super::*;
    use crate::guard::tests::FakeService;
    use crate::settings::{backup_dir, store_path};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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

    fn run(
        f: &Fixture,
        config: Option<&Config>,
        scope: EraseScope,
        service: &FakeService,
    ) -> Result<EraseReport, AppError> {
        erase_within(
            &f.paths,
            &f.transport,
            &f.config_path,
            config,
            scope,
            service,
            Duration::ZERO,
        )
    }

    #[test]
    fn erase_deletes_the_store_backups_and_logs_but_not_outputs_or_agent_files() -> TestResult {
        let f = fixture()?;
        let report = run(
            &f,
            Some(&f.config),
            EraseScope::default(),
            &FakeService::default(),
        )?;
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
    fn an_unloadable_config_erases_the_default_locations() -> TestResult {
        let f = fixture()?;
        run(&f, None, EraseScope::default(), &FakeService::default())?;
        assert!(!store_path(&f.paths, &f.config).exists());
        Ok(())
    }

    #[test]
    fn a_kept_service_is_stopped_then_started_again() -> TestResult {
        let f = fixture()?;
        let service = FakeService::loaded();
        run(&f, Some(&f.config), EraseScope::default(), &service)?;
        assert_eq!(*service.calls.borrow(), ["stop", "start"]);
        Ok(())
    }

    #[test]
    fn scope_adds_the_config_and_the_service() -> TestResult {
        let f = fixture()?;
        let service = FakeService::loaded();
        let report = run(
            &f,
            Some(&f.config),
            EraseScope {
                config: true,
                service: true,
            },
            &service,
        )?;
        assert!(!f.config_path.exists());
        assert_eq!(*service.calls.borrow(), ["uninstall"]);
        assert!(report.service_removed);
        Ok(())
    }

    #[test]
    fn erase_refuses_while_the_daemon_runs() -> TestResult {
        let f = fixture()?;
        let _daemon = f.transport.lock()?;
        let result = run(
            &f,
            Some(&f.config),
            EraseScope::default(),
            &FakeService::default(),
        );
        assert!(matches!(result, Err(AppError::DaemonRunning { .. })));
        assert!(store_path(&f.paths, &f.config).exists());
        Ok(())
    }
}
