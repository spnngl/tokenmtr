//! Background service management: the shared interface, and the daemon as a launchd `LaunchAgent`
//! (macOS). The Linux systemd user unit lives in [`crate::systemd`].

use std::fs::OpenOptions;
use std::io;
use std::process::Command;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_brand::LAUNCHD_LABEL as LABEL;

use crate::fs::replace_atomically;
use crate::paths::Paths;

/// Absolute so a hostile `PATH` cannot substitute the tool.
const LAUNCHCTL: &str = "/bin/launchctl";
/// Absolute so a hostile `PATH` cannot substitute the tool.
const ID: &str = "/usr/bin/id";
/// `bootout` returns before launchd has removed the job; how long to wait for it.
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll period while waiting for the job to disappear.
const UNLOAD_POLL: Duration = Duration::from_millis(100);
/// Minimum seconds between two launches; spaces out restarts of a daemon that cannot start.
const THROTTLE_SECS: u32 = 30;
/// File names of the daemon's launchd stdout and stderr, inside the logs dir.
const LAUNCHD_LOGS: [&str; 2] = ["launchd.out.log", "launchd.err.log"];
/// Size above which [`trim_launchd_logs`] empties a launchd log.
const LAUNCHD_LOG_LIMIT: u64 = 1024 * 1024;

/// Failure of a [`ServiceManager`] operation.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The daemon binary path is relative; the service manager would resolve it against `/`.
    #[error("daemon binary path must be absolute: {0}")]
    RelativeBinary(Utf8PathBuf),
    /// The daemon binary is on a disk image or in a translocated copy that disappears; the
    /// app must be moved to a stable folder first.
    #[error("{0} runs from a disk image or a temporary copy: move TokenMtr to Applications")]
    TransientBinary(Utf8PathBuf),
    /// The daemon binary path holds a control character, which a unit file cannot carry.
    #[error("daemon binary path holds a control character: {0:?}")]
    ControlCharacter(Utf8PathBuf),
    /// The service manager loads the unit from somewhere other than this home directory, as in a
    /// sandbox `HOME` that shares the real user manager; acting on it would touch the user's own
    /// daemon.
    #[error("the service manager does not load {expected} (it loads {loaded:?})")]
    ForeignUnit {
        /// The unit file of this installation.
        expected: Utf8PathBuf,
        /// What the manager loads instead; `None` when it knows no such unit.
        loaded: Option<String>,
    },
    /// Filesystem error while reading or writing the plist or unit file.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// An external command could not run to success.
    #[error("`{command}` failed (exit {code:?}): {stderr}")]
    Command {
        /// Command line, for diagnostics.
        command: String,
        /// Exit code, `None` if killed by a signal.
        code: Option<i32>,
        /// Trimmed standard error.
        stderr: String,
    },
    /// `id -u` printed something that is not a user id.
    #[error("cannot parse user id from {0:?}")]
    BadUid(String),
    /// launchd still lists the job after `bootout` and [`UNLOAD_TIMEOUT`].
    #[error("launchd did not unload {LABEL} within {} s", UNLOAD_TIMEOUT.as_secs())]
    StillLoaded,
}

/// State of the installed service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    /// No plist or unit file, and not loaded.
    NotInstalled,
    /// Plist or unit file present but not loaded (e.g. before the next login).
    Stopped,
    /// Loaded without a live process (starting, or waiting out a restart throttle).
    Loaded,
    /// Loaded with a live process.
    Running {
        /// Process id of the daemon.
        pid: u32,
    },
}

/// Installs and queries the background daemon.
pub trait ServiceManager {
    /// Installs and starts the service running `binary run`. Idempotent: an existing install is
    /// replaced and reloaded.
    ///
    /// # Errors
    /// [`ServiceError`] if `binary` is relative, the plist cannot be written, or the service
    /// manager rejects it.
    fn install(&self, binary: &Utf8Path) -> Result<(), ServiceError>;

    /// Stops and removes the service. Succeeds if it is not installed.
    ///
    /// # Errors
    /// [`ServiceError`] if the loaded service cannot be stopped or the plist cannot be removed.
    fn uninstall(&self) -> Result<(), ServiceError>;

    /// Unloads the service, keeping it installed, and waits until the service manager no longer
    /// lists it (the daemon process has exited). Returns whether it was loaded. A service that is
    /// not installed for these paths is left alone, even when a job with the same label runs
    /// (another `HOME`, e.g. a test sandbox, must never stop the user's daemon).
    ///
    /// # Errors
    /// [`ServiceError`] if the service cannot be unloaded or is still listed after a timeout.
    fn stop(&self) -> Result<bool, ServiceError>;

    /// Loads the installed service again after [`ServiceManager::stop`]. A no-op when it is
    /// already loaded.
    ///
    /// # Errors
    /// [`ServiceError`] if the service manager rejects it, e.g. because it is not installed.
    fn start(&self) -> Result<(), ServiceError>;

    /// Current state.
    ///
    /// # Errors
    /// [`ServiceError`] if the service manager cannot be queried.
    fn status(&self) -> Result<ServiceStatus, ServiceError>;
}

/// Result of a finished external command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code, `None` if killed by a signal.
    pub code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

impl CommandOutput {
    pub(crate) fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs external programs; faked in tests.
pub trait CommandRunner {
    /// Runs `program` with `args` to completion.
    ///
    /// # Errors
    /// An I/O error if the program cannot be spawned. A non-zero exit is not an error here.
    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput>;
}

/// [`CommandRunner`] backed by [`std::process::Command`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
        let out = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// [`ServiceManager`] for a per-user launchd agent.
#[derive(Debug)]
pub struct LaunchdService<R = SystemRunner> {
    runner: R,
    plist_path: Utf8PathBuf,
    logs_dir: Utf8PathBuf,
    unload_timeout: Duration,
}

impl<R: CommandRunner> LaunchdService<R> {
    /// The plist goes to `~/Library/LaunchAgents`; daemon stdout/stderr go to the logs dir.
    #[must_use]
    pub fn new(paths: &Paths, runner: R) -> Self {
        Self {
            runner,
            plist_path: paths
                .home()
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
            logs_dir: paths.logs_dir().to_owned(),
            unload_timeout: UNLOAD_TIMEOUT,
        }
    }

    /// Where the plist is written.
    #[must_use]
    pub fn plist_path(&self) -> &Utf8Path {
        &self.plist_path
    }

    /// Whether an installed plist exists and differs from what [`ServiceManager::install`]
    /// would write for `expected_binary` (typically because the `.app` moved).
    /// `false` when no plist is installed.
    ///
    /// # Errors
    /// I/O errors other than "not found" while reading the plist.
    pub fn plist_needs_repair(&self, expected_binary: &Utf8Path) -> Result<bool, ServiceError> {
        match std::fs::read(&self.plist_path) {
            Ok(current) => {
                Ok(current != render_plist(expected_binary, &self.logs_dir).into_bytes())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Rewrites and reloads the service if [`LaunchdService::plist_needs_repair`]. Returns
    /// whether it did.
    ///
    /// # Errors
    /// See [`LaunchdService::plist_needs_repair`] and [`ServiceManager::install`].
    pub fn repair(&self, expected_binary: &Utf8Path) -> Result<bool, ServiceError> {
        let needed = self.plist_needs_repair(expected_binary)?;
        if needed {
            self.install(expected_binary)?;
        }
        Ok(needed)
    }

    fn launchctl(&self, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        Ok(self.runner.run(LAUNCHCTL, args)?)
    }

    fn loaded(&self, target: &str) -> Result<bool, ServiceError> {
        Ok(self.launchctl(&["print", target])?.success())
    }

    /// `bootout` when loaded, then waits until launchd no longer lists the job: `bootstrap`
    /// right after an asynchronous `bootout` fails with "5: Input/output error".
    fn unload(&self, target: &str) -> Result<bool, ServiceError> {
        if !self.loaded(target)? {
            return Ok(false);
        }
        self.launchctl_checked(&["bootout", target])?;
        let deadline = Instant::now() + self.unload_timeout;
        while self.loaded(target)? {
            if Instant::now() >= deadline {
                return Err(ServiceError::StillLoaded);
            }
            std::thread::sleep(UNLOAD_POLL);
        }
        Ok(true)
    }

    fn target(&self) -> Result<String, ServiceError> {
        Ok(format!("{}/{LABEL}", self.domain()?))
    }

    fn launchctl_checked(&self, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        let out = self.launchctl(args)?;
        if out.success() {
            Ok(out)
        } else {
            Err(ServiceError::Command {
                command: format!("launchctl {}", args.join(" ")),
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            })
        }
    }

    /// `gui/<uid>`, the launchd domain of the logged-in user. The uid comes from `id -u` so this
    /// crate needs no extra `unsafe`.
    fn domain(&self) -> Result<String, ServiceError> {
        let out = self.runner.run(ID, &["-u"])?;
        let uid = out.stdout.trim();
        match (out.success(), uid.parse::<u32>()) {
            (true, Ok(uid)) => Ok(format!("gui/{uid}")),
            _ => Err(ServiceError::BadUid(uid.to_owned())),
        }
    }
}

/// Whether `binary` lives where it will not stay: a mounted disk image (`/Volumes/…`) or the
/// read-only copy Gatekeeper runs a quarantined app from (`…/AppTranslocation/…`).
#[must_use]
pub fn is_transient(binary: &Utf8Path) -> bool {
    binary.starts_with("/Volumes")
        || binary
            .components()
            .any(|part| part.as_str() == "AppTranslocation")
}

impl<R: CommandRunner> ServiceManager for LaunchdService<R> {
    fn install(&self, binary: &Utf8Path) -> Result<(), ServiceError> {
        if !binary.is_absolute() {
            return Err(ServiceError::RelativeBinary(binary.to_owned()));
        }
        if is_transient(binary) {
            return Err(ServiceError::TransientBinary(binary.to_owned()));
        }
        if let Some(dir) = self.plist_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::create_dir_all(&self.logs_dir)?;
        replace_atomically(
            &self.plist_path,
            render_plist(binary, &self.logs_dir).as_bytes(),
        )?;

        let domain = self.domain()?;
        // `bootstrap` fails on an already loaded service, so reload explicitly.
        self.unload(&format!("{domain}/{LABEL}"))?;
        self.launchctl_checked(&["bootstrap", &domain, self.plist_path.as_str()])?;
        Ok(())
    }

    fn uninstall(&self) -> Result<(), ServiceError> {
        self.unload(&self.target()?)?;
        match std::fs::remove_file(&self.plist_path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn stop(&self) -> Result<bool, ServiceError> {
        if !self.plist_path.exists() {
            return Ok(false);
        }
        self.unload(&self.target()?)
    }

    fn start(&self) -> Result<(), ServiceError> {
        let domain = self.domain()?;
        if self.loaded(&format!("{domain}/{LABEL}"))? {
            return Ok(());
        }
        self.launchctl_checked(&["bootstrap", &domain, self.plist_path.as_str()])?;
        Ok(())
    }

    fn status(&self) -> Result<ServiceStatus, ServiceError> {
        let target = self.target()?;
        let printed = self.launchctl(&["print", &target])?;
        if !printed.success() {
            return Ok(if self.plist_path.exists() {
                ServiceStatus::Stopped
            } else {
                ServiceStatus::NotInstalled
            });
        }
        Ok(match parse_pid(&printed.stdout) {
            Some(pid) => ServiceStatus::Running { pid },
            None => ServiceStatus::Loaded,
        })
    }
}

/// Extracts the daemon pid from `launchctl print` output (a `pid = N` line, present only while
/// the process is alive).
fn parse_pid(print_output: &str) -> Option<u32> {
    print_output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = ")?.trim().parse().ok())
}

/// Empties the daemon's launchd stdout/stderr logs in `logs_dir` once they exceed 1 MiB;
/// launchd never rotates them. launchd opens them in append mode, so truncating a log the
/// running daemon writes to is safe. Missing logs are skipped.
///
/// # Errors
/// I/O errors other than "not found".
pub fn trim_launchd_logs(logs_dir: &Utf8Path) -> io::Result<()> {
    for name in LAUNCHD_LOGS {
        let file = match OpenOptions::new().write(true).open(logs_dir.join(name)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() > LAUNCHD_LOG_LIMIT {
            file.set_len(0)?;
        }
    }
    Ok(())
}

/// Renders the `LaunchAgent` plist. Priority is per thread, so `ProcessType` stays
/// `Standard` and no `LowPriorityIO` is set.
///
/// launchd restarts the daemon only after a failed exit: a clean exit (after `Shutdown`, or when
/// another instance already serves) stays down, so erase and restore can stop it.
fn render_plist(binary: &Utf8Path, logs_dir: &Utf8Path) -> String {
    let binary = xml_escape(binary.as_str());
    let [out_log, err_log] = LAUNCHD_LOGS.map(|name| xml_escape(logs_dir.join(name).as_str()));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{binary}</string>
		<string>run</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>{THROTTLE_SECS}</integer>
	<key>ProcessType</key>
	<string>Standard</string>
	<key>StandardOutPath</key>
	<string>{out_log}</string>
	<key>StandardErrorPath</key>
	<string>{err_log}</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Emulates `id` and the three `launchctl` verbs, recording every call.
    #[derive(Default)]
    struct FakeRunner {
        loaded: Mutex<bool>,
        calls: Mutex<Vec<String>>,
        pid_line: Option<&'static str>,
        fail_bootstrap: bool,
        /// `bootout` succeeds but the job never leaves.
        stuck: bool,
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
            let ok = |stdout: &str| CommandOutput {
                code: Some(0),
                stdout: stdout.to_owned(),
                stderr: String::new(),
            };
            let fail = |code| CommandOutput {
                code: Some(code),
                stdout: String::new(),
                stderr: "boom\n".to_owned(),
            };
            let mut loaded = self.loaded.lock().unwrap();
            Ok(match (program, args.first().copied()) {
                (ID, _) => ok("501\n"),
                (LAUNCHCTL, Some("print")) if *loaded => {
                    ok(self.pid_line.unwrap_or("state = waiting\n"))
                }
                (LAUNCHCTL, Some("print")) => fail(113),
                (LAUNCHCTL, Some("bootstrap")) if self.fail_bootstrap => fail(5),
                (LAUNCHCTL, Some("bootstrap")) => {
                    *loaded = true;
                    ok("")
                }
                (LAUNCHCTL, Some("bootout")) if self.stuck => ok(""),
                (LAUNCHCTL, Some("bootout")) => {
                    *loaded = false;
                    ok("")
                }
                _ => fail(127),
            })
        }
    }

    fn setup(runner: &FakeRunner) -> (tempfile::TempDir, LaunchdService<&FakeRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_str().unwrap().to_owned();
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        (dir, LaunchdService::new(&paths, runner))
    }

    const BINARY: &str = "/Applications/TokenMtr.app/Contents/MacOS/tokenmtr";

    #[test]
    fn plist_golden() {
        let plist = render_plist(
            Utf8Path::new(BINARY),
            Utf8Path::new("/Users/me/Library/Logs/tokenmtr"),
        );
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>dev.tokenmtr.daemon</string>
	<key>ProgramArguments</key>
	<array>
		<string>/Applications/TokenMtr.app/Contents/MacOS/tokenmtr</string>
		<string>run</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>30</integer>
	<key>ProcessType</key>
	<string>Standard</string>
	<key>StandardOutPath</key>
	<string>/Users/me/Library/Logs/tokenmtr/launchd.out.log</string>
	<key>StandardErrorPath</key>
	<string>/Users/me/Library/Logs/tokenmtr/launchd.err.log</string>
</dict>
</plist>
"#;
        assert_eq!(plist, expected);
    }

    #[test]
    fn plist_escapes_xml_in_paths() {
        let plist = render_plist(Utf8Path::new("/A&B/<x>/'q'\"d\""), Utf8Path::new("/l"));
        assert!(plist.contains("<string>/A&amp;B/&lt;x&gt;/&apos;q&apos;&quot;d&quot;</string>"));
    }

    #[test]
    fn install_writes_plist_and_bootstraps() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        service.install(Utf8Path::new(BINARY)).unwrap();

        let written = std::fs::read_to_string(service.plist_path()).unwrap();
        assert_eq!(
            written,
            render_plist(Utf8Path::new(BINARY), &service.logs_dir)
        );
        assert!(service.logs_dir.is_dir());
        assert_eq!(
            runner.calls(),
            [
                "/usr/bin/id -u".to_owned(),
                "/bin/launchctl print gui/501/dev.tokenmtr.daemon".to_owned(),
                format!("/bin/launchctl bootstrap gui/501 {}", service.plist_path()),
            ]
        );
    }

    #[test]
    fn reinstall_waits_for_the_bootout_before_bootstrapping() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        service.install(Utf8Path::new(BINARY)).unwrap();
        service.install(Utf8Path::new(BINARY)).unwrap();
        let calls = runner.calls();
        let target = "gui/501/dev.tokenmtr.daemon";
        assert_eq!(
            calls[calls.len() - 3..],
            [
                format!("/bin/launchctl bootout {target}"),
                format!("/bin/launchctl print {target}"),
                format!("/bin/launchctl bootstrap gui/501 {}", service.plist_path()),
            ]
        );
        assert!(*runner.loaded.lock().unwrap());
    }

    #[test]
    fn stop_leaves_a_job_installed_for_another_home_alone() {
        let runner = FakeRunner {
            loaded: Mutex::new(true),
            ..FakeRunner::default()
        };
        let (_dir, service) = setup(&runner);
        assert!(!service.stop().unwrap());
        assert_eq!(runner.calls(), Vec::<String>::new());
    }

    #[test]
    fn stop_then_start_keeps_the_install() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        assert!(!service.stop().unwrap());

        service.install(Utf8Path::new(BINARY)).unwrap();
        assert!(service.stop().unwrap());
        assert_eq!(service.status().unwrap(), ServiceStatus::Stopped);
        service.start().unwrap();
        assert_eq!(service.status().unwrap(), ServiceStatus::Loaded);
        let bootstraps = runner
            .calls()
            .iter()
            .filter(|c| c.contains("bootstrap"))
            .count();
        service.start().unwrap();
        let again = runner
            .calls()
            .iter()
            .filter(|c| c.contains("bootstrap"))
            .count();
        assert_eq!(bootstraps, again, "start on a loaded service is a no-op");
    }

    #[test]
    fn a_job_that_never_leaves_times_out() {
        let runner = FakeRunner {
            loaded: Mutex::new(true),
            stuck: true,
            ..FakeRunner::default()
        };
        let (_dir, mut service) = setup(&runner);
        service.unload_timeout = Duration::from_millis(250);
        std::fs::create_dir_all(service.plist_path.parent().unwrap()).unwrap();
        std::fs::write(&service.plist_path, b"").unwrap();
        assert!(matches!(service.stop(), Err(ServiceError::StillLoaded)));
    }

    #[test]
    fn launchd_logs_are_emptied_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Utf8Path::from_path(dir.path()).unwrap();
        trim_launchd_logs(logs).unwrap(); // missing logs are fine
        let big = vec![b'x'; usize::try_from(LAUNCHD_LOG_LIMIT).unwrap() + 1];
        std::fs::write(logs.join(LAUNCHD_LOGS[0]), &big).unwrap();
        std::fs::write(logs.join(LAUNCHD_LOGS[1]), b"small").unwrap();
        trim_launchd_logs(logs).unwrap();
        assert_eq!(
            std::fs::metadata(logs.join(LAUNCHD_LOGS[0])).unwrap().len(),
            0
        );
        assert_eq!(std::fs::read(logs.join(LAUNCHD_LOGS[1])).unwrap(), b"small");
    }

    #[test]
    fn install_rejects_relative_binary_and_surfaces_launchctl_failure() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        assert!(matches!(
            service.install(Utf8Path::new("tokenmtr")),
            Err(ServiceError::RelativeBinary(_))
        ));
        for transient in [
            "/Volumes/TokenMtr/TokenMtr.app/Contents/MacOS/tokenmtr",
            "/private/var/folders/x/AppTranslocation/ABC/d/TokenMtr.app/Contents/MacOS/tokenmtr",
        ] {
            assert!(matches!(
                service.install(Utf8Path::new(transient)),
                Err(ServiceError::TransientBinary(_))
            ));
        }
        assert!(!is_transient(Utf8Path::new(BINARY)));
        assert_eq!(runner.calls().len(), 0);

        let runner = FakeRunner {
            fail_bootstrap: true,
            ..FakeRunner::default()
        };
        let (_dir, service) = setup(&runner);
        let err = service.install(Utf8Path::new(BINARY)).unwrap_err();
        assert!(
            matches!(err, ServiceError::Command { code: Some(5), .. }),
            "{err}"
        );
    }

    #[test]
    fn uninstall_removes_plist_and_unloads() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        service.install(Utf8Path::new(BINARY)).unwrap();
        service.uninstall().unwrap();
        assert!(!service.plist_path().exists());
        assert!(!*runner.loaded.lock().unwrap());
        assert_eq!(service.status().unwrap(), ServiceStatus::NotInstalled);
    }

    #[test]
    fn uninstall_when_not_installed_is_ok() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        service.uninstall().unwrap();
        assert!(runner.calls().iter().all(|c| !c.contains("bootout")));
    }

    #[test]
    fn status_covers_every_state() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        assert_eq!(service.status().unwrap(), ServiceStatus::NotInstalled);

        service.install(Utf8Path::new(BINARY)).unwrap();
        assert_eq!(service.status().unwrap(), ServiceStatus::Loaded);

        *runner.loaded.lock().unwrap() = false;
        assert_eq!(service.status().unwrap(), ServiceStatus::Stopped);

        let runner = FakeRunner {
            pid_line: Some("\tstate = running\n\tpid = 4242\n"),
            ..FakeRunner::default()
        };
        let (_dir, service) = setup(&runner);
        service.install(Utf8Path::new(BINARY)).unwrap();
        assert_eq!(
            service.status().unwrap(),
            ServiceStatus::Running { pid: 4242 }
        );
    }

    #[test]
    fn repair_follows_a_moved_binary() {
        let runner = FakeRunner::default();
        let (_dir, service) = setup(&runner);
        let moved = Utf8Path::new("/Users/me/Apps/TokenMtr.app/Contents/MacOS/tokenmtr");

        // Not installed: nothing to repair.
        assert!(!service.plist_needs_repair(moved).unwrap());
        assert!(!service.repair(moved).unwrap());

        service.install(Utf8Path::new(BINARY)).unwrap();
        assert!(!service.plist_needs_repair(Utf8Path::new(BINARY)).unwrap());
        assert!(service.plist_needs_repair(moved).unwrap());

        assert!(service.repair(moved).unwrap());
        let written = std::fs::read_to_string(service.plist_path()).unwrap();
        assert!(written.contains(moved.as_str()));
        assert!(!service.plist_needs_repair(moved).unwrap());
    }

    #[test]
    fn parse_pid_ignores_unrelated_lines() {
        assert_eq!(parse_pid("a = 1\n\tpid = 7\n"), Some(7));
        assert_eq!(parse_pid("state = waiting\nlast pid = x\n"), None);
    }
}
