//! Control channel transport (D24): a Unix domain socket at `<data_dir>/control.sock`, guarded
//! by the daemon lock `<data_dir>/daemon.lock`.
//!
//! The transport carries bytes only; [`crate::framing`] splits them into lines and the control
//! crate owns the message types.
//!
//! One lock serialises everything that must not overlap with a running daemon: the daemon
//! holds it for its whole life, and offline writers (erase, restore, direct scans and backups)
//! hold it for the duration of their operation. The lock is an `flock` on an open file, so the
//! kernel releases it when the holder exits, even after a crash.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};

use camino::{Utf8Path, Utf8PathBuf};
use tokio::net::{UnixListener, UnixStream};

/// One control connection (either side).
pub type ControlStream = UnixStream;

/// File name of the daemon lock, next to the socket.
const LOCK_NAME: &str = "daemon.lock";

/// Failure to take the daemon lock or to listen on the control socket.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// A daemon holds the lock and accepts connections on the socket.
    #[error("another daemon is already listening on {0}")]
    AlreadyRunning(Utf8PathBuf),
    /// Another process holds the daemon lock: a daemon that is starting or stopping, or an
    /// offline operation such as erase or restore.
    #[error("the daemon lock {0} is held by another process")]
    Locked(Utf8PathBuf),
    /// Filesystem or socket error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Exclusive ownership of the data directory against a running daemon. Released on drop.
#[derive(Debug)]
pub struct DaemonLock {
    _file: File,
}

/// Server side of the control channel. Holds the [`DaemonLock`]; dropping it removes the socket
/// file, then releases the lock, so a successor never loses its fresh socket to a late cleanup.
#[derive(Debug)]
pub struct ControlListener {
    // Declared first: fields drop in order, so the socket goes before the lock.
    bound: Bound,
    lock: DaemonLock,
}

/// The listening socket; dropping it removes the socket file.
#[derive(Debug)]
struct Bound {
    listener: UnixListener,
    socket_path: Utf8PathBuf,
    owner_uid: u32,
}

impl ControlListener {
    /// Waits for the next client running as the socket owner. Connections from any other user
    /// (possible only if the socket permissions were loosened) are closed and logged.
    ///
    /// # Errors
    /// Errors from the underlying `accept`.
    pub async fn accept(&self) -> io::Result<ControlStream> {
        loop {
            let (stream, _) = self.bound.listener.accept().await?;
            match stream.peer_cred() {
                Ok(peer) if peer.uid() == self.bound.owner_uid => return Ok(stream),
                Ok(peer) => tracing::warn!(uid = peer.uid(), "control: rejected foreign user"),
                Err(e) => tracing::warn!(error = %e, "control: cannot read peer credentials"),
            }
        }
    }

    /// Stops listening and removes the socket file but keeps the lock, so the caller finishes
    /// its cleanup before a successor can start.
    #[must_use]
    pub fn close(self) -> DaemonLock {
        let Self { bound, lock } = self;
        drop(bound);
        lock
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.socket_path)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, socket = %self.socket_path, "control: cannot remove socket");
        }
    }
}

/// Where and how the control channel is reached. Cheap to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlTransport {
    socket_path: Utf8PathBuf,
}

impl ControlTransport {
    /// A Unix-domain-socket transport at `socket_path`.
    ///
    /// The path must fit `sun_path` (104 bytes on macOS); longer paths fail at bind/connect.
    #[must_use]
    pub fn unix(socket_path: Utf8PathBuf) -> Self {
        Self { socket_path }
    }

    /// Socket file path.
    #[must_use]
    pub fn socket_path(&self) -> &Utf8Path {
        &self.socket_path
    }

    /// Daemon lock file path, next to the socket.
    #[must_use]
    pub fn lock_path(&self) -> Utf8PathBuf {
        self.socket_path.with_file_name(LOCK_NAME)
    }

    /// Takes the daemon lock without waiting, creating the parent directory if needed and
    /// restricting it to its owner (`0700`).
    ///
    /// # Errors
    /// [`ControlError::Locked`] if another process holds it; [`ControlError::Io`] otherwise.
    pub fn lock(&self) -> Result<DaemonLock, ControlError> {
        if let Some(parent) = self.socket_path.parent().filter(|p| !p.as_str().is_empty()) {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = self.lock_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(DaemonLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(ControlError::Locked(path)),
            Err(TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// Takes the daemon lock (see [`ControlTransport::lock`]), then listens with the socket
    /// restricted to its owner (`0600`).
    ///
    /// Holding the lock makes any socket file left behind a crashed daemon's, so it is replaced.
    /// A path that exists but is not a socket is never deleted.
    ///
    /// # Errors
    /// [`ControlError::AlreadyRunning`] if a live daemon owns the lock and socket;
    /// [`ControlError::Locked`] if another process holds the lock; [`ControlError::Io`]
    /// otherwise.
    pub async fn bind(&self) -> Result<ControlListener, ControlError> {
        let lock = match self.lock() {
            Ok(lock) => lock,
            Err(ControlError::Locked(path)) => {
                return Err(if self.connect().await.is_ok() {
                    ControlError::AlreadyRunning(self.socket_path.clone())
                } else {
                    ControlError::Locked(path)
                });
            }
            Err(e) => return Err(e),
        };
        // A daemon from before the lock existed does not hold it; never steal its socket.
        if self.connect().await.is_ok() {
            return Err(ControlError::AlreadyRunning(self.socket_path.clone()));
        }
        match std::fs::symlink_metadata(&self.socket_path) {
            Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(&self.socket_path)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", self.socket_path),
                )
                .into());
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // The parent is `0700` (see `lock`), so no other user can reach the socket before the
        // chmod below; `accept` checks the peer uid on top.
        // Wrapped at once so an error below still removes the socket file.
        let mut bound = Bound {
            listener: UnixListener::bind(&self.socket_path)?,
            socket_path: self.socket_path.clone(),
            owner_uid: u32::MAX,
        };
        std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600))?;
        bound.owner_uid = std::fs::metadata(&self.socket_path)?.uid();
        Ok(ControlListener { bound, lock })
    }

    /// Connects to the daemon.
    ///
    /// # Errors
    /// `NotFound` or `ConnectionRefused` when no daemon is listening.
    pub async fn connect(&self) -> io::Result<ControlStream> {
        UnixStream::connect(&self.socket_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::{LineReader, LineWriter};

    fn transport() -> (tempfile::TempDir, ControlTransport) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("data/control.sock")).unwrap();
        (dir, ControlTransport::unix(path))
    }

    fn mode(path: &Utf8Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn socket_is_owner_only_and_carries_lines() {
        let (_guard, transport) = transport();
        let listener = transport.bind().await.unwrap();
        assert_eq!(mode(transport.socket_path()), 0o600);
        assert_eq!(mode(transport.socket_path().parent().unwrap()), 0o700);

        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let (read, write) = stream.into_split();
            let line = LineReader::new(read).read_line().await.unwrap().unwrap();
            LineWriter::new(write)
                .write_line(&format!("echo {line}"))
                .await
                .unwrap();
        });

        let (read, write) = transport.connect().await.unwrap().into_split();
        LineWriter::new(write).write_line("ping").await.unwrap();
        let reply = LineReader::new(read).read_line().await.unwrap();
        assert_eq!(reply.as_deref(), Some("echo ping"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_existing_parent_is_tightened() {
        let (_guard, transport) = transport();
        let parent = transport.socket_path().parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _listener = transport.bind().await.unwrap();
        assert_eq!(mode(parent), 0o700);
    }

    #[tokio::test]
    async fn stale_socket_is_replaced() {
        let (_guard, transport) = transport();
        std::fs::create_dir_all(transport.socket_path().parent().unwrap()).unwrap();
        // A crashed daemon leaves its socket file behind.
        drop(std::os::unix::net::UnixListener::bind(transport.socket_path()).unwrap());
        assert!(transport.socket_path().exists());
        let _listener = transport.bind().await.unwrap();
        transport.connect().await.unwrap();
    }

    #[tokio::test]
    async fn dropping_the_listener_removes_the_socket_and_frees_the_lock() {
        let (_guard, transport) = transport();
        drop(transport.bind().await.unwrap());
        assert!(!transport.socket_path().exists());
        drop(transport.lock().unwrap());
    }

    #[tokio::test]
    async fn closing_keeps_the_lock_until_dropped() {
        let (_guard, transport) = transport();
        let lock = transport.bind().await.unwrap().close();
        assert!(!transport.socket_path().exists());
        assert!(matches!(transport.lock(), Err(ControlError::Locked(_))));
        drop(lock);
        drop(transport.lock().unwrap());
    }

    #[tokio::test]
    async fn live_listener_is_not_stolen() {
        let (_guard, transport) = transport();
        let _listener = transport.bind().await.unwrap();
        assert!(matches!(
            transport.bind().await,
            Err(ControlError::AlreadyRunning(_))
        ));
        assert!(matches!(transport.lock(), Err(ControlError::Locked(_))));
    }

    #[tokio::test]
    async fn a_held_lock_blocks_the_daemon_without_a_listener() {
        let (_guard, transport) = transport();
        let _lock = transport.lock().unwrap();
        assert!(matches!(
            transport.bind().await,
            Err(ControlError::Locked(path)) if path == transport.lock_path()
        ));
        assert!(!transport.socket_path().exists());
    }

    #[tokio::test]
    async fn regular_file_is_never_deleted() {
        let (_guard, transport) = transport();
        let parent = transport.socket_path().parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        std::fs::write(transport.socket_path(), b"precious").unwrap();
        assert!(transport.bind().await.is_err());
        assert_eq!(std::fs::read(transport.socket_path()).unwrap(), b"precious");
    }

    #[tokio::test]
    async fn connect_without_daemon_fails() {
        let (_guard, transport) = transport();
        assert!(transport.connect().await.is_err());
    }
}
