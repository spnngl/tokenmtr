//! `tokenmtr service`: the launchd background service (D23).

use anyhow::{Context, bail};
use camino::Utf8PathBuf;
use clap::Subcommand;
use tokenmtr_platform::{Platform, ServiceManager, ServiceStatus};

use crate::setup::Setup;

/// What `tokenmtr service` does.
#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    /// Install the service and start it; reinstalls a service that is already loaded.
    Install {
        /// The binary the service runs; this executable when absent.
        #[arg(long)]
        bin: Option<Utf8PathBuf>,
    },
    /// Stop the service and remove it.
    Uninstall,
    /// Show whether the service is installed and running.
    Status,
}

/// `tokenmtr service install|uninstall|status`.
pub fn service(setup: &Setup, action: &ServiceAction) -> anyhow::Result<()> {
    println!("{}", run(setup.platform().service(), action)?);
    Ok(())
}

/// Runs `action` against `manager` and returns the line to print.
fn run(manager: &dyn ServiceManager, action: &ServiceAction) -> anyhow::Result<String> {
    match action {
        ServiceAction::Install { bin } => {
            let bin = match bin {
                Some(bin) => bin.clone(),
                None => current_exe()?,
            };
            manager
                .install(&bin)
                .with_context(|| format!("cannot install the service for {bin}"))?;
            Ok(format!("service installed: {bin}"))
        }
        ServiceAction::Uninstall => {
            manager.uninstall().context("cannot remove the service")?;
            Ok("service removed".to_owned())
        }
        ServiceAction::Status => Ok(match manager.status().context("cannot read the service")? {
            ServiceStatus::NotInstalled => "service not installed".to_owned(),
            ServiceStatus::Stopped => "service installed, not loaded".to_owned(),
            ServiceStatus::Loaded => "service loaded, not running".to_owned(),
            ServiceStatus::Running { pid } => format!("service running (pid {pid})"),
        }),
    }
}

fn current_exe() -> anyhow::Result<Utf8PathBuf> {
    let exe = std::env::current_exe().context("cannot find this executable")?;
    match Utf8PathBuf::from_path_buf(exe) {
        Ok(path) => Ok(path),
        Err(path) => bail!("{} is not valid UTF-8", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;

    use tokenmtr_platform::{CommandOutput, CommandRunner, LaunchdService, Paths};

    use super::*;

    /// Emulates `id` and `launchctl`, recording every call.
    #[derive(Default)]
    struct FakeRunner {
        loaded: Mutex<bool>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for &FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            let reply = |code, stdout: &str| CommandOutput {
                code: Some(code),
                stdout: stdout.to_owned(),
                stderr: String::new(),
            };
            let mut loaded = self.loaded.lock().unwrap();
            Ok(match (program, args.first().copied()) {
                ("/usr/bin/id", _) => reply(0, "501\n"),
                ("/bin/launchctl", Some("print")) if *loaded => reply(0, "pid = 4242\n"),
                ("/bin/launchctl", Some("print")) => reply(113, ""),
                ("/bin/launchctl", Some("bootstrap")) => {
                    *loaded = true;
                    reply(0, "")
                }
                ("/bin/launchctl", Some("bootout")) => {
                    *loaded = false;
                    reply(0, "")
                }
                _ => reply(127, ""),
            })
        }
    }

    #[test]
    fn install_status_uninstall_drive_launchctl() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_str().unwrap().to_owned();
        let paths = Paths::from_lookup(|key| (key == "HOME").then(|| root.clone())).unwrap();
        let runner = FakeRunner::default();
        let manager = LaunchdService::new(&paths, &runner);
        let bin = Utf8PathBuf::from("/usr/local/bin/tokenmtr");

        let line = run(&manager, &ServiceAction::Install { bin: Some(bin) }).unwrap();
        assert_eq!(line, "service installed: /usr/local/bin/tokenmtr");
        assert!(manager.plist_path().exists());
        assert!(
            runner
                .calls()
                .iter()
                .any(|call| call.starts_with("/bin/launchctl bootstrap gui/501"))
        );

        let status = run(&manager, &ServiceAction::Status).unwrap();
        assert_eq!(status, "service running (pid 4242)");

        run(&manager, &ServiceAction::Uninstall).unwrap();
        assert!(!manager.plist_path().exists());
        let status = run(&manager, &ServiceAction::Status).unwrap();
        assert_eq!(status, "service not installed");
    }

    #[test]
    fn install_refuses_a_relative_binary() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_str().unwrap().to_owned();
        let paths = Paths::from_lookup(|key| (key == "HOME").then(|| root.clone())).unwrap();
        let runner = FakeRunner::default();
        let manager = LaunchdService::new(&paths, &runner);
        let bin = Utf8PathBuf::from("tokenmtr");
        let error = run(&manager, &ServiceAction::Install { bin: Some(bin) }).unwrap_err();
        assert!(format!("{error:#}").contains("tokenmtr"), "{error:#}");
    }
}
