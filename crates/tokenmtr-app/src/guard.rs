//! Actions that need the only writer (D1) gone: the guard, and restore.

use std::os::unix::net::UnixStream;

use camino::Utf8Path;
use tokenmtr_platform::ControlTransport;

use crate::error::AppError;

/// Whether a daemon accepts connections on its control socket.
#[must_use]
pub fn daemon_running(transport: &ControlTransport) -> bool {
    UnixStream::connect(transport.socket_path()).is_ok()
}

/// # Errors
///
/// [`AppError::DaemonRunning`] when a daemon answers on `transport`.
pub fn ensure_daemon_stopped(transport: &ControlTransport) -> Result<(), AppError> {
    if daemon_running(transport) {
        return Err(AppError::DaemonRunning {
            socket: transport.socket_path().to_owned(),
        });
    }
    Ok(())
}

/// Replaces the store at `store_path` with the backup `backup` (D37): decompress next to it,
/// `integrity_check`, refuse a newer schema (D52), drop the WAL files, rename into place.
///
/// # Errors
///
/// [`AppError::DaemonRunning`] while a daemon runs; [`AppError::Store`] when the backup is
/// unreadable, corrupt or newer. The current store is untouched then.
pub fn restore_backup(
    transport: &ControlTransport,
    store_path: &Utf8Path,
    backup: &Utf8Path,
) -> Result<(), AppError> {
    ensure_daemon_stopped(transport)?;
    tokenmtr_store::restore(backup, store_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use camino::Utf8PathBuf;
    use tokenmtr_store::{Store, StoreRead, StoreReader};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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
            ensure_daemon_stopped(&transport),
            Err(AppError::DaemonRunning { .. })
        ));
        Ok(())
    }

    #[test]
    fn restore_refuses_while_the_daemon_runs_and_keeps_the_store() -> TestResult {
        let dir = tempfile::tempdir()?;
        let transport = transport(&dir)?;
        let _listener = UnixListener::bind(transport.socket_path())?;
        let store_path = Utf8PathBuf::from_path_buf(dir.path().join("usage.db"))
            .map_err(|_| "temp dir is not UTF-8")?;
        drop(Store::open(&store_path)?);
        let before = std::fs::read(&store_path)?;
        let result = restore_backup(&transport, &store_path, Utf8Path::new("/nonexistent.zst"));
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
        restore_backup(&transport, &store_path, &backup)?;
        drop(StoreReader::open(&store_path)?);
        Ok(())
    }
}
