//! Control channel transport (D24): a Unix domain socket at `<data_dir>/control.sock`.
//!
//! The transport carries bytes only; [`crate::framing`] splits them into lines and the control
//! crate owns the message types.

use std::io;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use tokio::net::{UnixListener, UnixStream};

/// Server side of the control channel; yields one [`ControlStream`] per client.
pub type ControlListener = UnixListener;
/// One control connection (either side).
pub type ControlStream = UnixStream;

/// Failure to start listening on the control socket.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// Another process is accepting connections on the socket.
    #[error("another daemon is already listening on {0}")]
    AlreadyRunning(Utf8PathBuf),
    /// Filesystem or socket error.
    #[error(transparent)]
    Io(#[from] io::Error),
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

    /// Starts listening, creating missing parent directories (mode `0700`) and restricting the
    /// socket file to its owner (`0600`).
    ///
    /// A leftover socket file from a crashed daemon (nobody accepts on it) is replaced. A path
    /// that exists but is not a socket is never deleted.
    ///
    /// # Errors
    /// [`ControlError::AlreadyRunning`] if a live listener owns the path; [`ControlError::Io`]
    /// otherwise.
    pub async fn bind(&self) -> Result<ControlListener, ControlError> {
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .await?;
        }
        match UnixStream::connect(&self.socket_path).await {
            Ok(_) => return Err(ControlError::AlreadyRunning(self.socket_path.clone())),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                tokio::fs::remove_file(&self.socket_path).await?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(&self.socket_path)?;
        tokio::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600))
            .await?;
        Ok(listener)
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
            let (stream, _) = listener.accept().await.unwrap();
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
    async fn stale_socket_is_replaced() {
        let (_guard, transport) = transport();
        drop(transport.bind().await.unwrap()); // listener gone, file remains
        assert!(transport.socket_path().exists());
        let _listener = transport.bind().await.unwrap();
        transport.connect().await.unwrap();
    }

    #[tokio::test]
    async fn live_listener_is_not_stolen() {
        let (_guard, transport) = transport();
        let _listener = transport.bind().await.unwrap();
        assert!(matches!(
            transport.bind().await,
            Err(ControlError::AlreadyRunning(_))
        ));
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
