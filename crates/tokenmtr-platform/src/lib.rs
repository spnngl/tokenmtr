//! OS integration: default locations, background service, control socket, secrets, thread
//! priority and durable file writes. macOS is the only implemented platform (PLAN.md §4).

pub mod control;
pub mod framing;
pub mod fs;
pub mod paths;
pub mod qos;
pub mod secrets;
pub mod service;

pub use control::{ControlError, ControlListener, ControlStream, ControlTransport};
pub use framing::{FrameError, LineReader, LineWriter, MAX_LINE_BYTES};
pub use paths::{Paths, PathsError};
pub use qos::{ThreadPriority, set_thread_priority};
#[cfg(target_os = "macos")]
pub use secrets::KeychainSecrets;
pub use secrets::{SecretError, SecretStore};
pub use service::{
    CommandOutput, CommandRunner, LaunchdService, ServiceError, ServiceManager, ServiceStatus,
    SystemRunner,
};

/// Everything the daemon, CLI and app need from the operating system.
pub trait Platform {
    /// Config, data, logs and cache locations.
    fn paths(&self) -> &Paths;
    /// Installs and queries the background service.
    fn service(&self) -> &dyn ServiceManager;
    /// How the control channel is reached.
    fn control_transport(&self) -> ControlTransport;
    /// Read-only credential store.
    fn secrets(&self) -> &dyn SecretStore;
    /// Sets the calling thread's priority. Best effort: a failure is logged and the thread keeps
    /// its current priority.
    fn set_thread_priority(&self, priority: ThreadPriority);
}

/// macOS: launchd, Unix socket, Keychain, `QoS` classes.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct MacOs {
    paths: Paths,
    service: LaunchdService,
    secrets: KeychainSecrets,
}

#[cfg(target_os = "macos")]
impl MacOs {
    /// Builds the macOS platform over `paths`.
    #[must_use]
    pub fn new(paths: Paths) -> Self {
        Self {
            service: LaunchdService::new(&paths, SystemRunner),
            secrets: KeychainSecrets,
            paths,
        }
    }

    /// The concrete launchd service, for plist repair (D23), which is not part of
    /// [`ServiceManager`].
    #[must_use]
    pub fn launchd(&self) -> &LaunchdService {
        &self.service
    }
}

#[cfg(target_os = "macos")]
impl Platform for MacOs {
    fn paths(&self) -> &Paths {
        &self.paths
    }

    fn service(&self) -> &dyn ServiceManager {
        &self.service
    }

    fn control_transport(&self) -> ControlTransport {
        ControlTransport::unix(self.paths.data_dir().join("control.sock"))
    }

    fn secrets(&self) -> &dyn SecretStore {
        &self.secrets
    }

    fn set_thread_priority(&self, priority: ThreadPriority) {
        if let Err(error) = qos::set_thread_priority(priority) {
            tracing::warn!(?priority, %error, "cannot set thread priority");
        }
    }
}

/// The platform the program runs on; one variant per supported OS.
#[derive(Debug)]
pub enum PlatformKind {
    /// macOS.
    #[cfg(target_os = "macos")]
    MacOs(MacOs),
}

impl PlatformKind {
    /// The platform of the running OS over the default [`Paths::from_env`].
    ///
    /// # Errors
    /// [`PathsError`] if the environment does not yield valid paths.
    #[cfg(target_os = "macos")]
    pub fn detect() -> Result<Self, PathsError> {
        Ok(Self::MacOs(MacOs::new(Paths::from_env()?)))
    }
}

impl Platform for PlatformKind {
    fn paths(&self) -> &Paths {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs(p) => p.paths(),
        }
    }

    fn service(&self) -> &dyn ServiceManager {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs(p) => p.service(),
        }
    }

    fn control_transport(&self) -> ControlTransport {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs(p) => p.control_transport(),
        }
    }

    fn secrets(&self) -> &dyn SecretStore {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs(p) => p.secrets(),
        }
    }

    fn set_thread_priority(&self, priority: ThreadPriority) {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs(p) => p.set_thread_priority(priority),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_platform_wires_paths_into_every_part() {
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| "/Users/me".to_owned())).unwrap();
        let platform = PlatformKind::MacOs(MacOs::new(paths));
        assert_eq!(
            platform.control_transport().socket_path(),
            "/Users/me/Library/Application Support/dev.tokenmtr.app/control.sock"
        );
        assert_eq!(
            platform.paths().logs_dir(),
            "/Users/me/Library/Logs/tokenmtr"
        );
    }
}
