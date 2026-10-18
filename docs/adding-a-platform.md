# Adding a platform

A platform supplies the five OS services in the `Platform` trait. Everything else (ingest, store,
export, UI) is platform-neutral. Work through the steps in order and finish each one before the
next. macOS (`MacOs`) is the reference: copy its shape.

All code lives in `crates/tokenmtr-platform/src/`. Read first: [`lib.rs`](../crates/tokenmtr-platform/src/lib.rs)
(the trait, `MacOs`, `PlatformKind`).

```rust
pub trait Platform {
    fn paths(&self) -> &Paths;
    fn service(&self) -> &dyn ServiceManager;
    fn control_transport(&self) -> ControlTransport;
    fn secrets(&self) -> &dyn SecretStore;
    fn set_thread_priority(&self, priority: ThreadPriority);
}
```

## 1. Paths (D21, D50)

[`paths.rs`](../crates/tokenmtr-platform/src/paths.rs). `Paths::from_lookup` builds the macOS layout
(`~/Library/Application Support/dev.tokenmtr.app`, `~/Library/Logs/tokenmtr`, ...). Give the new
platform its own constructor that fills the same fields from its native conventions (XDG on Linux,
`%APPDATA%` on Windows), and keep these rules:

- `TOKENMTR_CONFIG` still overrides only the config file; `XDG_CONFIG_HOME` is honoured only when absolute.
- `exports_dir()` stays under `data_dir()`, outside every protected folder.
- `protected()` and `is_protected()` list the folders the OS guards with consent prompts, which
  `doctor` and `/settings` read. `protected()` returns a fixed `[Utf8PathBuf; 5]` today: change it to
  a `Vec` for an OS with another count, or none.
- The constructor takes an injectable lookup (`impl Fn(&str) -> Option<String>`), never reads the
  process environment directly, so tests need no global state.

## 2. Service manager (D23)

[`service.rs`](../crates/tokenmtr-platform/src/service.rs). Implement `ServiceManager`:

| Method | Contract |
|---|---|
| `install(&self, binary: &Utf8Path)` | Reject a relative `binary` (`ServiceError::RelativeBinary`). Write the unit atomically (`fs::replace_atomically`), create the logs dir, start the service running `<binary> run`. Idempotent: replace and reload an existing install. |
| `uninstall(&self)` | Stop, then delete the unit. Succeeds when nothing is installed. |
| `status(&self)` | `NotInstalled`, `Stopped` (unit present, not loaded), `Loaded` (no process), `Running { pid }`. |

The service must start at login, restart after a crash, and keep the default process priority:
priority is per thread (step 5).

Take external commands through the `CommandRunner` trait, never `std::process::Command` directly, so
tests script the answers. launchd example (`LaunchdService<R: CommandRunner>`):

```rust
// install: write plist, boot out a loaded copy, bootstrap the new one.
replace_atomically(&self.plist_path, render_plist(binary, &self.logs_dir).as_bytes())?;
let domain = self.domain()?;                       // gui/<uid>, from `id -u`
let target = format!("{domain}/{LABEL}");          // LABEL = "dev.tokenmtr.daemon"
if self.launchctl(&["print", &target])?.success() {
    self.launchctl_checked(&["bootout", &target])?;
}
self.launchctl_checked(&["bootstrap", &domain, self.plist_path.as_str()])?;
```

The plist (`render_plist`) sets `RunAtLoad`, `KeepAlive` and `ProcessType = Standard`, and escapes
XML in every path.

Also expose the plist-path repair the desktop app runs on launch (`plist_needs_repair`, `repair`):
a moved app breaks `ProgramArguments`. These are not on the trait; reach them through the concrete
type, as `MacOs::launchd()` does.

## 3. Control transport (D24)

[`control.rs`](../crates/tokenmtr-platform/src/control.rs). `ControlTransport` is a Unix domain
socket at `<data_dir>/control.sock`, created `0600` in a `0700` directory; `bind` replaces a stale
socket and refuses a live one (`ControlError::AlreadyRunning`). A Unix-like platform reuses it. A
platform without Unix sockets (Windows) adds a variant for its transport (named pipe) behind
`ControlTransport`, keeping `bind` and `connect`, and restricts access to the current user.

## 4. Secrets (D26, D55)

[`secrets.rs`](../crates/tokenmtr-platform/src/secrets.rs). Implement `SecretStore::get(&self, service: &str) -> Result<Option<SecretString>, SecretError>`:

- Read-only; never write to the store.
- A missing item is `Ok(None)`; a locked or refused store is `SecretError::Store(message)` with no
  secret in the message; non-text data is `SecretError::NotUtf8`.
- The call may block and may show an OS consent prompt: callers use a blocking context.
- Return `secrecy::SecretString`, never `String`.

Gate the implementation and its dependency by target: `#[cfg(target_os = "...")]` on the module and
`[target.'cfg(target_os = "...")'.dependencies]` in `Cargo.toml`.

## 5. Thread priority (D47)

[`qos.rs`](../crates/tokenmtr-platform/src/qos.rs). Add an `imp` module for the target that maps
`ThreadPriority::Background` to the OS's low-power, throttled-I/O class and `UserInitiated` to its
interactive class (macOS: `pthread_set_qos_class_self_np`). Best effort: return the OS error; the
`Platform` method logs it and keeps the current priority. Targets without a mapping keep the
existing no-op `imp`.

## 6. Wire it up

1. Add a cfg-gated variant to `PlatformKind` and a `<Name>` struct implementing `Platform`, next to `MacOs`.
2. Add a match arm for the variant in every `Platform` method of `PlatformKind`.
3. Add the target's `detect()` that builds it over its `Paths` constructor.
4. Keep `replace_atomically` ([`fs.rs`](../crates/tokenmtr-platform/src/fs.rs), D49) as the only way to
   write files that matter: temp file, `sync_all`, `rename`, parent directory sync. Port the
   directory sync if the OS needs a different call.
5. Run the platform tests in `.github/workflows/ci.yml` on a runner of the new OS.

## 7. Tests

Every test below runs without touching the real system.

- **Paths:** `from_lookup` with a fake environment: defaults, an absolute and a relative
  `XDG_CONFIG_HOME`, `TOKENMTR_CONFIG`, a missing `HOME`, and `is_protected` on nested and
  differently-cased paths.
- **Service:** a `FakeRunner` implementing `CommandRunner` records calls and scripts answers. Assert
  a golden unit file, install then reinstall (boot out first), a relative binary, a failing command
  (`ServiceError::Command` carries stderr), uninstall when not installed, and every `ServiceStatus`.
- **Control:** bind in a temp dir; check the file mode, stale-socket replacement and
  `AlreadyRunning`.
- **Secrets:** a test creates its own item, reads it back, and removes it on drop; a missing item is `None`.
- **Priority:** read the class back from the calling thread after `set_thread_priority`.

## Checklist

- [ ] `Paths` constructor with injectable lookup; protected folders listed or empty.
- [ ] `ServiceManager`: install, uninstall, status; starts at login; restarts after a crash.
- [ ] Control transport with owner-only access.
- [ ] `SecretStore`: read-only, `SecretString`, no secret in errors.
- [ ] `ThreadPriority` mapping or documented no-op.
- [ ] `PlatformKind` variant, match arms, `detect()`.
- [ ] Tests from step 7; `just check` passes.
- [ ] `docs/install.md` has the platform's install, permissions and uninstall steps.

**Done when:** on the new OS, after `tokenmtr service install` the daemon runs, survives logout and
login, restarts after `kill -9`, and `uninstall` leaves no unit file or process behind; the secret
store reads a test item created by the test itself; the daemon's background threads run at the low
priority class and switch to the interactive class during `ScanNow`; and `just check` passes.
