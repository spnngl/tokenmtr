# Adding a platform

A platform supplies the five OS services in the `Platform` trait. Everything else (ingest, store,
export, UI) is platform-neutral. Work through the steps in order and finish each one, its **Done
when** included, before the next. macOS (`MacOs`) is the reference: copy its shape.

All code lives in `crates/tokenmtr-platform/src/`. Read first: [`lib.rs`](../crates/tokenmtr-platform/src/lib.rs)
(the `Platform` trait, `MacOs`, `PlatformKind`). The trait has five methods: `paths`, `service`,
`control_transport`, `secrets` and `set_thread_priority`; their doc comments are the contract.

## 1. Paths

[`paths.rs`](../crates/tokenmtr-platform/src/paths.rs). `Paths::from_lookup` builds the macOS layout
(`~/Library/Application Support/dev.tokenmtr.app`, `~/Library/Logs/tokenmtr`, ...). Give the new
platform its own constructor that fills the same fields from its native conventions (XDG on Linux,
`%APPDATA%` on Windows), and keep these rules:

- The config file is `$TOKENMTR_CONFIG` (absolute only), else `$XDG_CONFIG_HOME/tokenmtr/config.toml`
  when `XDG_CONFIG_HOME` is absolute, else `~/.config/tokenmtr/config.toml`.
- `exports_dir()` stays under `data_dir()`, outside every protected folder.
- `protected()` and `is_protected()` list the folders the OS guards with consent prompts, which
  `doctor` and `/settings` read. `protected()` returns a `Vec<Utf8PathBuf>`: empty for an OS without
  such folders.
- The constructor takes an injectable lookup (`impl Fn(&str) -> Option<String>`) instead of the
  process environment, so tests need no global state.

**Done when:** the paths tests of step 7 pass for the new constructor.

## 2. Service manager

[`service.rs`](../crates/tokenmtr-platform/src/service.rs). Implement `ServiceManager`; its doc
comments are the contract:

| Method | Contract |
|---|---|
| `install(&self, binary: &Utf8Path)` | Reject a relative `binary` (`ServiceError::RelativeBinary`) and one that disappears, such as a mounted image (`ServiceError::TransientBinary`; macOS: `is_transient`). Write the unit atomically (`fs::replace_atomically`), create the logs dir, start the service running `<binary> run`. Idempotent: replace and reload an existing install. |
| `uninstall(&self)` | Stop, then delete the unit. Succeeds when nothing is installed. |
| `stop(&self)` | Unload, keep the unit, wait until the process is gone; return whether it was loaded. Leave a same-named job of another `HOME` alone. |
| `start(&self)` | Load the installed unit again; a no-op when loaded. |
| `status(&self)` | `NotInstalled`, `Stopped` (unit present, not loaded), `Loaded` (no process), `Running { pid }`. |

The service starts at login and restarts only after a crash or a failed exit: the daemon exits 0
after `shutdown` and when another instance already runs, and must stay down then (launchd:
`KeepAlive { SuccessfulExit = false }`, `ThrottleInterval = 30`). It keeps the default process
priority, because priority is per thread (step 5).

Take external commands through the `CommandRunner` trait instead of `std::process::Command`, so
tests script the answers. `LaunchdService<R = SystemRunner>` is the example: `install` writes the
plist (`render_plist`, XML-escaped paths), boots out a loaded copy and polls until launchd drops
it, then bootstraps the new one; `launchctl` and `id` are called by absolute path.

Also expose the unit-path repair the desktop app runs on launch (`plist_needs_repair`, `repair`):
a moved app breaks `ProgramArguments`. These are not on the trait; reach them through the concrete
type, as `MacOs::launchd()` does.

**Done when:** the service tests of step 7 pass with a fake runner.

## 3. Control transport

[`control.rs`](../crates/tokenmtr-platform/src/control.rs). `ControlTransport` is a Unix domain
socket at `<data_dir>/control.sock`, created `0600` in a `0700` directory; `bind` replaces a stale
socket and refuses a live one (`ControlError::AlreadyRunning`). A Unix-like platform reuses it. A
platform without Unix sockets (Windows) adds a variant for its transport (named pipe) behind
`ControlTransport`, keeping `bind` and `connect`, and restricts access to the current user. The
wire protocol is in [`control-protocol.md`](control-protocol.md).

**Done when:** a client connects to a bound transport, and a second `bind` returns `AlreadyRunning`.

## 4. Secrets

[`secrets.rs`](../crates/tokenmtr-platform/src/secrets.rs). Implement `SecretStore::get(&self, service: &str) -> Result<Option<SecretString>, SecretError>`:

- Read-only: the store is Claude Code's, TokenMtr only reads it.
- A missing item is `Ok(None)`; a lookup that would need the user's consent is `SecretError::Denied`
  (the lookup never shows a prompt); any other store failure is `SecretError::Store(message)` with
  no secret in the message; non-text data is `SecretError::NotUtf8`.
- The call may block: callers use a blocking context.
- Return `secrecy::SecretString`.

Gate the implementation and its dependency by target: `#[cfg(target_os = "...")]` on the module and
`[target.'cfg(target_os = "...")'.dependencies]` in `Cargo.toml`.

**Done when:** the secrets test of step 7 reads back an item it created, and a missing item is `None`.

## 5. Thread priority

[`qos.rs`](../crates/tokenmtr-platform/src/qos.rs). Add an `imp` module for the target that maps
`ThreadPriority::Background` to the OS's low-power, throttled-I/O class and `UserInitiated` to its
interactive class (macOS: `pthread_set_qos_class_self_np`). Best effort: return the OS error; the
`Platform` method logs it and keeps the current priority. Targets without a mapping keep the
existing no-op `imp`.

**Done when:** the priority test of step 7 reads back the class it set.

## 6. Wire it up

1. Add a cfg-gated variant to `PlatformKind` and a `<Name>` struct implementing `Platform`, next to `MacOs`.
2. Add a match arm for the variant in every `Platform` method of `PlatformKind`.
3. Add the target's `PlatformKind::detect()` that builds it over its `Paths` constructor.
4. Write every file that matters through `replace_atomically` ([`fs.rs`](../crates/tokenmtr-platform/src/fs.rs)):
   temp file, `sync_all`, `rename`, parent directory sync. Port the directory sync if the OS needs
   a different call.
5. Add a job on a runner of the new OS to `.github/workflows/ci.yml`.

**Done when:** `cargo build --workspace` succeeds on the new OS and CI runs its tests there.

## 7. Tests

Every test below runs without touching the real system.

- **Paths:** `from_lookup` with a fake environment: defaults, an absolute and a relative
  `XDG_CONFIG_HOME`, `TOKENMTR_CONFIG`, a missing `HOME`, and `is_protected` on nested and
  differently-cased paths.
- **Service:** a fake `CommandRunner` records calls and scripts answers. Assert a golden unit file,
  install then reinstall (boot out first), a relative and a transient binary, a failing command
  (`ServiceError::Command` carries stderr), uninstall when not installed, stop and start, and every
  `ServiceStatus`.
- **Control:** bind in a temp dir; check the file mode, stale-socket replacement and
  `AlreadyRunning`.
- **Secrets:** a test creates its own item, reads it back, and removes it on drop; a missing item is `None`.
- **Priority:** read the class back from the calling thread after `set_thread_priority`.

**Done when:** `just check` passes on the new OS.

## Finish

Add the platform's install, permissions and uninstall steps to [`install.md`](install.md).

**Done when:** on the new OS, after `tokenmtr service install` the daemon runs, survives logout and
login, restarts after `kill -9`, stays down after `shutdown`, and `uninstall` leaves no unit file or
process behind; the secret store reads a test item created by the test itself; the daemon's
background threads run at the low priority class and switch to the interactive class during
`ScanNow`; and `just check` passes.
