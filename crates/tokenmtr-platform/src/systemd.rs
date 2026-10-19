//! Background service management on Linux: the daemon runs as a systemd user unit.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_brand::{DISPLAY_NAME, SYSTEMD_UNIT as UNIT};

use crate::fs::replace_atomically;
use crate::paths::Paths;
use crate::service::{
    CommandOutput, CommandRunner, ServiceError, ServiceManager, ServiceStatus, SystemRunner,
};

/// Absolute so a hostile `PATH` cannot substitute the tool.
const SYSTEMCTL: &str = "/usr/bin/systemctl";
/// Seconds between a failed exit and the restart; spaces out restarts of a daemon that cannot
/// start.
const RESTART_SEC: u32 = 30;
/// Properties read by [`SystemdUserService::properties`].
const PROPERTIES: &str = "FragmentPath,ActiveState,MainPID";

/// What `systemctl --user show` says about the unit.
#[derive(Debug, Default, PartialEq, Eq)]
struct Properties {
    /// File the manager loaded the unit from; empty when it knows no such unit.
    fragment_path: String,
    active_state: String,
    main_pid: u32,
}

impl Properties {
    fn parse(show_output: &str) -> Self {
        let mut properties = Self::default();
        for (key, value) in show_output.lines().filter_map(|line| line.split_once('=')) {
            match key {
                "FragmentPath" => value.clone_into(&mut properties.fragment_path),
                "ActiveState" => value.clone_into(&mut properties.active_state),
                "MainPID" => properties.main_pid = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        properties
    }

    /// Whether a process runs, is starting, is stopping or waits for a restart.
    fn is_live(&self) -> bool {
        matches!(
            self.active_state.as_str(),
            "active" | "reloading" | "activating" | "deactivating"
        )
    }
}

/// [`ServiceManager`] for a per-user systemd unit.
#[derive(Debug)]
pub struct SystemdUserService<R = SystemRunner> {
    runner: R,
    unit_path: Utf8PathBuf,
}

impl<R: CommandRunner> SystemdUserService<R> {
    /// The unit goes to `$XDG_CONFIG_HOME/systemd/user` (else `~/.config/systemd/user`); the
    /// daemon's output goes to the journal (`journalctl --user -u tokenmtr`).
    #[must_use]
    pub fn new(paths: &Paths, runner: R) -> Self {
        Self {
            runner,
            unit_path: paths.config_home().join("systemd/user").join(UNIT),
        }
    }

    /// Where the unit file is written.
    #[must_use]
    pub fn unit_path(&self) -> &Utf8Path {
        &self.unit_path
    }

    /// `systemctl --user <args>`, failing on a non-zero exit. Without a user manager (container,
    /// WSL, a cleared environment) this fails with "Failed to connect to bus".
    fn systemctl(&self, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        let mut full = vec!["--user"];
        full.extend_from_slice(args);
        let out = self.runner.run(SYSTEMCTL, &full)?;
        if out.success() {
            Ok(out)
        } else {
            Err(ServiceError::Command {
                command: format!("systemctl {}", full.join(" ")),
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            })
        }
    }

    fn properties(&self) -> Result<Properties, ServiceError> {
        let out = self.systemctl(&["show", UNIT, "--property", PROPERTIES])?;
        Ok(Properties::parse(&out.stdout))
    }

    /// The manager's view of the unit when it loads it from our file; `None` when it knows no
    /// such unit or loads another one. A sandbox `HOME` shares the real user manager, so it must
    /// never act on the unit of the user's own installation.
    fn owned_properties(&self) -> Result<Option<Properties>, ServiceError> {
        let properties = self.properties()?;
        Ok((properties.fragment_path == self.unit_path.as_str()).then_some(properties))
    }

    fn foreign(&self, loaded: &str) -> ServiceError {
        ServiceError::ForeignUnit {
            expected: self.unit_path.clone(),
            loaded: (!loaded.is_empty()).then(|| loaded.to_owned()),
        }
    }
}

impl<R: CommandRunner> ServiceManager for SystemdUserService<R> {
    fn install(&self, binary: &Utf8Path) -> Result<(), ServiceError> {
        let unit = render_unit(binary)?;
        if let Some(dir) = self.unit_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        replace_atomically(&self.unit_path, unit.as_bytes())?;

        self.systemctl(&["daemon-reload"])?;
        let loaded = self.properties()?.fragment_path;
        if loaded != self.unit_path.as_str() {
            return Err(self.foreign(&loaded));
        }
        self.systemctl(&["enable", UNIT])?;
        // `restart` also starts a stopped unit and replaces an outdated daemon.
        self.systemctl(&["restart", UNIT])?;
        Ok(())
    }

    fn uninstall(&self) -> Result<(), ServiceError> {
        if !self.unit_path.exists() {
            return Ok(());
        }
        if self.owned_properties()?.is_some() {
            self.systemctl(&["disable", "--now", UNIT])?;
        }
        match std::fs::remove_file(&self.unit_path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        self.systemctl(&["daemon-reload"])?;
        Ok(())
    }

    fn stop(&self) -> Result<bool, ServiceError> {
        if !self.unit_path.exists() {
            return Ok(false);
        }
        match self.owned_properties()? {
            Some(properties) if properties.is_live() => {
                // `stop` returns once the daemon has exited, and cancels a pending restart.
                self.systemctl(&["stop", UNIT])?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn start(&self) -> Result<(), ServiceError> {
        let properties = self.properties()?;
        if properties.fragment_path != self.unit_path.as_str() {
            return Err(self.foreign(&properties.fragment_path));
        }
        self.systemctl(&["start", UNIT])?;
        Ok(())
    }

    fn status(&self) -> Result<ServiceStatus, ServiceError> {
        if !self.unit_path.exists() {
            return Ok(ServiceStatus::NotInstalled);
        }
        let Some(properties) = self.owned_properties()? else {
            return Ok(ServiceStatus::Stopped);
        };
        Ok(match properties.active_state.as_str() {
            "active" | "reloading" if properties.main_pid > 0 => ServiceStatus::Running {
                pid: properties.main_pid,
            },
            "active" | "reloading" | "activating" | "deactivating" => ServiceStatus::Loaded,
            _ => ServiceStatus::Stopped,
        })
    }
}

/// Renders the unit file.
///
/// `Restart=on-failure` restarts the daemon only after a failed exit: a clean exit (after
/// `Shutdown`, or when another instance already serves) stays down, so erase and restore can
/// stop it. `ConditionFileIsExecutable` makes a unit whose package was removed skip quietly
/// instead of failing every [`RESTART_SEC`] seconds.
///
/// # Errors
/// [`ServiceError::RelativeBinary`] or [`ServiceError::ControlCharacter`] for a `binary` that
/// cannot be written into a unit.
fn render_unit(binary: &Utf8Path) -> Result<String, ServiceError> {
    if !binary.is_absolute() {
        return Err(ServiceError::RelativeBinary(binary.to_owned()));
    }
    if binary.as_str().chars().any(char::is_control) {
        return Err(ServiceError::ControlCharacter(binary.to_owned()));
    }
    let condition = binary.as_str().replace('%', "%%");
    let exec = exec_quote(binary.as_str());
    Ok(format!(
        "[Unit]
Description={DISPLAY_NAME} daemon
ConditionFileIsExecutable={condition}

[Service]
ExecStart={exec} run
Restart=on-failure
RestartSec={RESTART_SEC}

[Install]
WantedBy=default.target
"
    ))
}

/// Quotes `text` as one `ExecStart=` word: backslash and double quote are escaped, and `%` and
/// `$` doubled so systemd expands no specifier or variable.
fn exec_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const BINARY: &str = "/usr/bin/tokenmtr";

    /// Which unit the emulated user manager has loaded.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Loaded {
        /// The unit file of the service under test, once `daemon-reload` ran.
        Ours,
        /// Another installation's unit, as in a sandbox `HOME`.
        Foreign,
        Nothing,
    }

    /// Emulates `systemctl --user`, recording every call.
    struct FakeRunner {
        unit_path: Mutex<String>,
        loaded: Loaded,
        active_state: Mutex<&'static str>,
        main_pid: u32,
        /// Verb that exits 1 with "Failed to connect to bus".
        failing: Option<&'static str>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn new(loaded: Loaded, active_state: &'static str) -> Self {
            Self {
                unit_path: Mutex::default(),
                loaded,
                active_state: Mutex::new(active_state),
                main_pid: 4242,
                failing: None,
                calls: Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn state(&self) -> &'static str {
            *self.active_state.lock().unwrap()
        }
    }

    impl CommandRunner for &FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            let ok = |stdout: String| CommandOutput {
                code: Some(0),
                stdout,
                stderr: String::new(),
            };
            let verb = args.get(1).copied().unwrap_or_default();
            if program != SYSTEMCTL || args.first() != Some(&"--user") {
                return Ok(CommandOutput {
                    code: Some(127),
                    stdout: String::new(),
                    stderr: "not found\n".to_owned(),
                });
            }
            if self.failing == Some(verb) {
                return Ok(CommandOutput {
                    code: Some(1),
                    stdout: String::new(),
                    stderr: "Failed to connect to bus: No medium found\n".to_owned(),
                });
            }
            let mut state = self.active_state.lock().unwrap();
            Ok(match verb {
                "show" => {
                    let fragment = match self.loaded {
                        Loaded::Ours => self.unit_path.lock().unwrap().clone(),
                        Loaded::Foreign => {
                            "/home/other/.config/systemd/user/tokenmtr.service".to_owned()
                        }
                        Loaded::Nothing => String::new(),
                    };
                    ok(format!(
                        "MainPID={}\nActiveState={}\nFragmentPath={fragment}\n",
                        self.main_pid, *state
                    ))
                }
                "restart" | "start" => {
                    *state = "active";
                    ok(String::new())
                }
                "stop" | "disable" => {
                    *state = "inactive";
                    ok(String::new())
                }
                _ => ok(String::new()),
            })
        }
    }

    fn setup(runner: &FakeRunner) -> (tempfile::TempDir, SystemdUserService<&FakeRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_str().unwrap().to_owned();
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        let service = SystemdUserService::new(&paths, runner);
        service
            .unit_path()
            .as_str()
            .clone_into(&mut runner.unit_path.lock().unwrap());
        (dir, service)
    }

    /// Writes our unit file, as a previous `install` would have.
    fn write_unit(service: &SystemdUserService<&FakeRunner>) {
        std::fs::create_dir_all(service.unit_path().parent().unwrap()).unwrap();
        std::fs::write(
            service.unit_path(),
            render_unit(Utf8Path::new(BINARY)).unwrap(),
        )
        .unwrap();
    }

    fn systemctl_calls(runner: &FakeRunner) -> Vec<String> {
        runner
            .calls()
            .iter()
            .map(|c| {
                c.strip_prefix("/usr/bin/systemctl --user ")
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn unit_golden() {
        let expected = "[Unit]
Description=TokenMtr daemon
ConditionFileIsExecutable=/usr/bin/tokenmtr

[Service]
ExecStart=\"/usr/bin/tokenmtr\" run
Restart=on-failure
RestartSec=30

[Install]
WantedBy=default.target
";
        assert_eq!(render_unit(Utf8Path::new(BINARY)).unwrap(), expected);
    }

    #[test]
    fn unit_escapes_specifiers_variables_and_quotes() {
        let unit = render_unit(Utf8Path::new("/opt/a b/%h/$HOME/\"q\"\\x")).unwrap();
        assert!(unit.contains("ConditionFileIsExecutable=/opt/a b/%%h/$HOME/\"q\"\\x\n"));
        assert!(unit.contains("ExecStart=\"/opt/a b/%%h/$$HOME/\\\"q\\\"\\\\x\" run\n"));
    }

    #[test]
    fn unit_rejects_relative_and_control_characters() {
        assert!(matches!(
            render_unit(Utf8Path::new("tokenmtr")),
            Err(ServiceError::RelativeBinary(_))
        ));
        assert!(matches!(
            render_unit(Utf8Path::new("/usr/bin/to\nken")),
            Err(ServiceError::ControlCharacter(_))
        ));
        assert!(matches!(
            render_unit(Utf8Path::new("/usr/bin/to\tken")),
            Err(ServiceError::ControlCharacter(_))
        ));
    }

    #[test]
    fn unit_goes_to_xdg_config_home() {
        let paths = Paths::from_lookup(|k| match k {
            "HOME" => Some("/home/me".to_owned()),
            "XDG_CONFIG_HOME" => Some("/c".to_owned()),
            _ => None,
        })
        .unwrap();
        let runner = FakeRunner::new(Loaded::Ours, "inactive");
        let service = SystemdUserService::new(&paths, &runner);
        assert_eq!(service.unit_path(), "/c/systemd/user/tokenmtr.service");
    }

    #[test]
    fn install_writes_the_unit_then_reloads_enables_and_restarts() {
        let runner = FakeRunner::new(Loaded::Ours, "inactive");
        let (_dir, service) = setup(&runner);
        service.install(Utf8Path::new(BINARY)).unwrap();

        assert_eq!(
            std::fs::read_to_string(service.unit_path()).unwrap(),
            render_unit(Utf8Path::new(BINARY)).unwrap()
        );
        assert_eq!(
            systemctl_calls(&runner),
            [
                "daemon-reload",
                "show tokenmtr.service --property FragmentPath,ActiveState,MainPID",
                "enable tokenmtr.service",
                "restart tokenmtr.service",
            ]
        );
        assert_eq!(runner.state(), "active");
    }

    #[test]
    fn reinstall_replaces_the_unit() {
        let runner = FakeRunner::new(Loaded::Ours, "inactive");
        let (_dir, service) = setup(&runner);
        service.install(Utf8Path::new(BINARY)).unwrap();
        service
            .install(Utf8Path::new("/usr/local/bin/tokenmtr"))
            .unwrap();
        let written = std::fs::read_to_string(service.unit_path()).unwrap();
        assert!(written.contains("ExecStart=\"/usr/local/bin/tokenmtr\" run"));
        assert!(!written.contains("\"/usr/bin/tokenmtr\""));
        assert_eq!(
            systemctl_calls(&runner)
                .iter()
                .filter(|c| c.starts_with("restart"))
                .count(),
            2
        );
    }

    #[test]
    fn install_rejects_a_relative_binary_before_touching_anything() {
        let runner = FakeRunner::new(Loaded::Ours, "inactive");
        let (_dir, service) = setup(&runner);
        assert!(matches!(
            service.install(Utf8Path::new("tokenmtr")),
            Err(ServiceError::RelativeBinary(_))
        ));
        assert!(!service.unit_path().exists());
        assert_eq!(runner.calls().len(), 0);
    }

    #[test]
    fn install_refuses_to_enable_a_unit_the_manager_loads_elsewhere() {
        for loaded in [Loaded::Foreign, Loaded::Nothing] {
            let runner = FakeRunner::new(loaded, "active");
            let (_dir, service) = setup(&runner);
            assert!(matches!(
                service.install(Utf8Path::new(BINARY)),
                Err(ServiceError::ForeignUnit { .. })
            ));
            let calls = systemctl_calls(&runner);
            assert!(
                !calls
                    .iter()
                    .any(|c| c.starts_with("enable") || c.starts_with("restart"))
            );
            assert_eq!(runner.state(), "active");
        }
    }

    #[test]
    fn install_reports_a_missing_user_manager() {
        let mut runner = FakeRunner::new(Loaded::Ours, "inactive");
        runner.failing = Some("daemon-reload");
        let (_dir, service) = setup(&runner);
        let err = service.install(Utf8Path::new(BINARY)).unwrap_err();
        let ServiceError::Command {
            command,
            code,
            stderr,
        } = err
        else {
            panic!("expected a command error, got {err:?}");
        };
        assert_eq!(command, "systemctl --user daemon-reload");
        assert_eq!(code, Some(1));
        assert!(stderr.starts_with("Failed to connect to bus"));
    }

    #[test]
    fn uninstall_without_a_unit_runs_nothing() {
        let runner = FakeRunner::new(Loaded::Ours, "active");
        let (_dir, service) = setup(&runner);
        service.uninstall().unwrap();
        assert_eq!(runner.calls().len(), 0);
    }

    #[test]
    fn uninstall_disables_removes_and_reloads() {
        let runner = FakeRunner::new(Loaded::Ours, "active");
        let (_dir, service) = setup(&runner);
        write_unit(&service);
        service.uninstall().unwrap();
        assert!(!service.unit_path().exists());
        assert_eq!(
            systemctl_calls(&runner),
            [
                "show tokenmtr.service --property FragmentPath,ActiveState,MainPID",
                "disable --now tokenmtr.service",
                "daemon-reload",
            ]
        );
    }

    #[test]
    fn uninstall_leaves_a_foreign_unit_enabled() {
        let runner = FakeRunner::new(Loaded::Foreign, "active");
        let (_dir, service) = setup(&runner);
        write_unit(&service);
        service.uninstall().unwrap();
        assert!(!service.unit_path().exists());
        assert!(
            !systemctl_calls(&runner)
                .iter()
                .any(|c| c.starts_with("disable"))
        );
        assert_eq!(runner.state(), "active");
    }

    #[test]
    fn stop_reports_whether_the_unit_was_live() {
        for (state, was_live) in [
            ("active", true),
            ("activating", true),
            ("inactive", false),
            ("failed", false),
        ] {
            let runner = FakeRunner::new(Loaded::Ours, state);
            let (_dir, service) = setup(&runner);
            write_unit(&service);
            assert_eq!(service.stop().unwrap(), was_live, "{state}");
            assert_eq!(
                systemctl_calls(&runner)
                    .iter()
                    .any(|c| c.starts_with("stop")),
                was_live,
                "{state}"
            );
        }
    }

    #[test]
    fn stop_leaves_foreign_and_missing_units_alone() {
        let runner = FakeRunner::new(Loaded::Foreign, "active");
        let (_dir, service) = setup(&runner);
        assert!(!service.stop().unwrap(), "no unit file");
        assert_eq!(runner.calls().len(), 0);

        write_unit(&service);
        assert!(!service.stop().unwrap(), "foreign unit");
        assert_eq!(runner.state(), "active");
        assert!(
            !systemctl_calls(&runner)
                .iter()
                .any(|c| c.starts_with("stop"))
        );
    }

    #[test]
    fn start_starts_our_unit_and_refuses_another() {
        let runner = FakeRunner::new(Loaded::Ours, "inactive");
        let (_dir, service) = setup(&runner);
        service.start().unwrap();
        assert_eq!(runner.state(), "active");

        for loaded in [Loaded::Foreign, Loaded::Nothing] {
            let runner = FakeRunner::new(loaded, "inactive");
            let (_dir, service) = setup(&runner);
            assert!(matches!(
                service.start(),
                Err(ServiceError::ForeignUnit { .. })
            ));
            assert_eq!(runner.state(), "inactive");
        }
    }

    #[test]
    fn status_maps_every_state() {
        let runner = FakeRunner::new(Loaded::Ours, "inactive");
        let (_dir, service) = setup(&runner);
        assert_eq!(service.status().unwrap(), ServiceStatus::NotInstalled);
        assert_eq!(runner.calls().len(), 0);

        write_unit(&service);
        for (state, expected) in [
            ("active", ServiceStatus::Running { pid: 4242 }),
            ("reloading", ServiceStatus::Running { pid: 4242 }),
            ("activating", ServiceStatus::Loaded),
            ("deactivating", ServiceStatus::Loaded),
            ("inactive", ServiceStatus::Stopped),
            ("failed", ServiceStatus::Stopped),
        ] {
            *runner.active_state.lock().unwrap() = state;
            assert_eq!(service.status().unwrap(), expected, "{state}");
        }
    }

    #[test]
    fn active_without_a_process_is_loaded() {
        let mut runner = FakeRunner::new(Loaded::Ours, "active");
        runner.main_pid = 0;
        let (_dir, service) = setup(&runner);
        write_unit(&service);
        assert_eq!(service.status().unwrap(), ServiceStatus::Loaded);
    }

    #[test]
    fn status_of_a_foreign_unit_is_stopped() {
        let runner = FakeRunner::new(Loaded::Foreign, "active");
        let (_dir, service) = setup(&runner);
        write_unit(&service);
        assert_eq!(service.status().unwrap(), ServiceStatus::Stopped);
    }

    #[test]
    fn status_surfaces_a_missing_user_manager() {
        let mut runner = FakeRunner::new(Loaded::Ours, "active");
        runner.failing = Some("show");
        let (_dir, service) = setup(&runner);
        write_unit(&service);
        assert!(matches!(
            service.status(),
            Err(ServiceError::Command { .. })
        ));
    }

    #[test]
    fn properties_ignore_unknown_lines() {
        let parsed = Properties::parse("Foo\nMainPID=x\nActiveState=active\nFragmentPath=/a=b\n");
        assert_eq!(
            parsed,
            Properties {
                fragment_path: "/a=b".to_owned(),
                active_state: "active".to_owned(),
                main_pid: 0,
            }
        );
    }
}
