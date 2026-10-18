//! The daemon from the app's side: the bundled binary, the launchd plist, and the handshake.

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use tokenmtr_control::{Client, ClientError, ClientInfo};
use tokenmtr_platform::{
    CommandRunner, ControlTransport, LaunchdService, ServiceError, ServiceManager,
};

/// The name of the CLI binary the app bundles as its sidecar.
const SIDECAR_NAME: &str = "tokenmtr";

/// Who the app says it is in the control handshake.
#[must_use]
pub fn client_info() -> ClientInfo {
    ClientInfo {
        name: "tokenmtr-app".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        schema: Some(tokenmtr_store::SCHEMA_VERSION),
    }
}

/// Where the `tokenmtr` binary the daemon runs lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    path: Utf8PathBuf,
    bundled: bool,
}

impl Sidecar {
    /// The sidecar next to the app executable `exe`. It counts as bundled only inside a
    /// `*.app/Contents/MacOS` directory, so a development build never repoints the daemon of an
    /// installed app.
    #[must_use]
    pub fn beside(exe: &Utf8Path) -> Self {
        let dir = exe.parent();
        let bundled = dir.is_some_and(|dir| {
            dir.file_name() == Some("MacOS")
                && dir
                    .parent()
                    .and_then(Utf8Path::parent)
                    .and_then(Utf8Path::extension)
                    == Some("app")
        });
        Self {
            path: dir.unwrap_or(exe).join(SIDECAR_NAME),
            bundled,
        }
    }

    /// The sidecar of the running executable.
    ///
    /// # Errors
    ///
    /// When the executable path is unknown or not UTF-8.
    pub fn locate() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let exe = Utf8PathBuf::from_path_buf(exe)
            .map_err(|path| format!("executable path {} is not UTF-8", path.display()))?;
        Ok(Self::beside(&exe))
    }

    /// The binary path.
    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Whether it sits inside an `.app` bundle.
    #[must_use]
    pub const fn bundled(&self) -> bool {
        self.bundled
    }
}

/// What the launch-time plist check did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Not an app bundle, or no sidecar on disk: nothing to compare against.
    Skipped,
    /// No plist is installed (the daemon is installed from onboarding).
    NotInstalled,
    /// The plist already points at the sidecar.
    Current,
    /// The plist pointed elsewhere (the `.app` moved); it was rewritten and reloaded.
    Repaired,
}

/// Points an installed plist at the bundled sidecar again when it differs (D23).
///
/// # Errors
///
/// Reading the plist, or reinstalling the service, failed.
pub fn repair_plist<R: CommandRunner>(
    service: &LaunchdService<R>,
    sidecar: &Sidecar,
) -> Result<RepairOutcome, ServiceError> {
    if !sidecar.bundled() || !sidecar.path().is_file() {
        return Ok(RepairOutcome::Skipped);
    }
    if !service.plist_path().exists() {
        return Ok(RepairOutcome::NotInstalled);
    }
    Ok(if service.repair(sidecar.path())? {
        RepairOutcome::Repaired
    } else {
        RepairOutcome::Current
    })
}

/// Installs (or reinstalls) the service so it runs the sidecar; also what `restart_daemon` does.
///
/// # Errors
///
/// The sidecar is missing, or launchd refused.
pub fn install_service<R: CommandRunner>(
    service: &LaunchdService<R>,
    sidecar: &Sidecar,
) -> Result<(), String> {
    if !sidecar.path().is_file() {
        return Err(format!("the daemon binary {} is missing", sidecar.path()));
    }
    service
        .install(sidecar.path())
        .map_err(|error| error.to_string())
}

/// What the app knows about the daemon from one handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Link {
    /// Handshake succeeded.
    Connected {
        /// The daemon's package version.
        daemon_version: String,
    },
    /// Nobody answers.
    Offline,
    /// The daemon speaks another protocol or store schema (D52).
    Incompatible {
        /// The daemon's package version.
        daemon_version: String,
    },
}

impl Link {
    /// The link a failed connect means.
    #[must_use]
    pub fn from_error(error: &ClientError) -> Self {
        match error {
            ClientError::Incompatible { daemon_version, .. } => Self::Incompatible {
                daemon_version: daemon_version.clone(),
            },
            _ => Self::Offline,
        }
    }
}

/// Connects once and reports the link; the connection is dropped.
pub async fn probe(transport: &ControlTransport) -> Link {
    match Client::connect(transport, client_info()).await {
        Ok(client) => Link::Connected {
            daemon_version: client.daemon().daemon_version.clone(),
        },
        Err(error) => Link::from_error(&error),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;

    use tempfile::TempDir;
    use tokenmtr_platform::{CommandOutput, Paths};

    use super::*;

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<String>>,
    }

    impl CommandRunner for &FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(format!("{program} {}", args.join(" ")));
            }
            let (code, stdout) = match (program, args.first().copied()) {
                ("id", _) => (0, "501"),
                ("launchctl", Some("print")) => (1, ""),
                _ => (0, ""),
            };
            Ok(CommandOutput {
                code: Some(code),
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_owned()).unwrap_or_else(|_| Utf8PathBuf::from("/"))
    }

    struct Fixture {
        _dir: TempDir,
        home: Utf8PathBuf,
        paths: Paths,
    }

    fn fixture() -> io::Result<Fixture> {
        let dir = tempfile::Builder::new()
            .prefix("tm-daemon")
            .tempdir_in("/tmp")?;
        let home = utf8(dir.path());
        let lookup = home.clone();
        let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| lookup.to_string()))
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Fixture {
            _dir: dir,
            home,
            paths,
        })
    }

    fn bundle(home: &Utf8Path) -> io::Result<Sidecar> {
        let macos = home.join("TokenMtr.app/Contents/MacOS");
        std::fs::create_dir_all(&macos)?;
        std::fs::write(macos.join(SIDECAR_NAME), b"")?;
        Ok(Sidecar::beside(&macos.join("tokenmtr-desktop")))
    }

    #[test]
    fn sidecar_is_bundled_only_inside_an_app() {
        let inside = Sidecar::beside(Utf8Path::new(
            "/Applications/TokenMtr.app/Contents/MacOS/tokenmtr-desktop",
        ));
        assert!(inside.bundled());
        assert_eq!(
            inside.path(),
            "/Applications/TokenMtr.app/Contents/MacOS/tokenmtr"
        );
        let dev = Sidecar::beside(Utf8Path::new("/repo/target/debug/tokenmtr-desktop"));
        assert!(!dev.bundled());
        assert_eq!(dev.path(), "/repo/target/debug/tokenmtr");
    }

    #[test]
    fn repair_skips_outside_a_bundle_and_without_a_plist() -> io::Result<()> {
        let fixture = fixture()?;
        let runner = FakeRunner::default();
        let service = LaunchdService::new(&fixture.paths, &runner);
        let dev = Sidecar::beside(Utf8Path::new("/repo/target/debug/tokenmtr-desktop"));
        assert_eq!(
            repair_plist(&service, &dev).map_err(io::Error::other)?,
            RepairOutcome::Skipped
        );
        let sidecar = bundle(&fixture.home)?;
        assert_eq!(
            repair_plist(&service, &sidecar).map_err(io::Error::other)?,
            RepairOutcome::NotInstalled
        );
        Ok(())
    }

    #[test]
    fn repair_rewrites_a_plist_that_points_at_the_old_location() -> io::Result<()> {
        let fixture = fixture()?;
        let runner = FakeRunner::default();
        let service = LaunchdService::new(&fixture.paths, &runner);
        let sidecar = bundle(&fixture.home)?;
        // Installed from where the app used to be.
        service
            .install(Utf8Path::new("/Old/TokenMtr.app/Contents/MacOS/tokenmtr"))
            .map_err(io::Error::other)?;
        assert_eq!(
            repair_plist(&service, &sidecar).map_err(io::Error::other)?,
            RepairOutcome::Repaired
        );
        let plist = std::fs::read_to_string(service.plist_path())?;
        assert!(plist.contains(sidecar.path().as_str()), "{plist}");
        assert_eq!(
            repair_plist(&service, &sidecar).map_err(io::Error::other)?,
            RepairOutcome::Current
        );
        Ok(())
    }

    #[test]
    fn install_needs_the_sidecar_on_disk() -> io::Result<()> {
        let fixture = fixture()?;
        let runner = FakeRunner::default();
        let service = LaunchdService::new(&fixture.paths, &runner);
        let missing = Sidecar::beside(Utf8Path::new("/nowhere/TokenMtr.app/Contents/MacOS/x"));
        assert!(install_service(&service, &missing).is_err());
        let sidecar = bundle(&fixture.home)?;
        install_service(&service, &sidecar).map_err(io::Error::other)?;
        Ok(())
    }

    #[test]
    fn link_classifies_connect_failures() {
        let offline = ClientError::Closed;
        assert_eq!(Link::from_error(&offline), Link::Offline);
        let incompatible = ClientError::Incompatible {
            daemon_protocol: 2,
            daemon_version: "9.9.9".to_owned(),
            store_schema: 4,
            reason: "newer".to_owned(),
        };
        assert_eq!(
            Link::from_error(&incompatible),
            Link::Incompatible {
                daemon_version: "9.9.9".to_owned()
            }
        );
    }
}
