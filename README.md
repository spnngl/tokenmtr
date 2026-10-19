<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/tokenmtr-dark.svg">
    <img src="assets/brand/tokenmtr-light.svg" alt="TokenMtr" width="450" height="120">
  </picture>
</h1>

Tracks what your coding agents cost. A background daemon reads the agents' local session files, prices every request at API rates, compares that with what you actually pay (subscription fees included), and shows the result in a menu-bar app, a CLI, and CSV/JSONL/SQLite exports. Session data stays on your machine; only quota, FX-rate and price-list lookups use the network.

Screenshots: not added yet.

## Supported agents

- Claude Code (`claude`)
- pi (`pi`)

macOS only for now.

## Install

See [docs/install.md](docs/install.md).

## Quick start

```sh
tokenmtr service install   # start the daemon at login
tokenmtr status            # daemon and sink health, plus a recap per agent
```

The config file is `~/.config/tokenmtr/config.toml` unless `TOKENMTR_CONFIG` or an absolute `XDG_CONFIG_HOME` points elsewhere (`tokenmtr config path` prints it). Every key is optional; an empty file is valid. A minimal example that exports usage to CSV files:

```toml
[[sinks]]
id = "local-csv"
kind = "csv"
```

Check it with `tokenmtr config check`. All keys: [docs/config.md](docs/config.md).

## CLI

| Command | What it does |
| --- | --- |
| `run` | Run the daemon in the foreground. |
| `status` | Show daemon and sink health and a recap of every followed agent. |
| `recap` | Show the recap of one agent, or of all agents summed. |
| `report` | Write a client report for a month or a date range. |
| `alerts` | List the alerts that fired. |
| `backup` | Write a backup now, or list the backups. |
| `restore` | Replace the store with a backup; stops the daemon and restarts the background service afterwards. |
| `erase` | Delete the store, its backups and the logs (`--delete-config` also the config, `--service` also the background service). |
| `scan` | Ingest the agents' files now. |
| `reprice` | Re-estimate unpriced events with the active pricing snapshot (`--all`: every event). |
| `doctor` | Check the config, the store, the daemon and the agents' files. |
| `service` | Install, remove or inspect the background service. |
| `quota` | Show the latest quota windows and the detected plans; `quota connect` lets the daemon read the Claude credentials in the Keychain. |
| `fx` | Show the latest FX rate. |
| `pricing` | Show the active pricing snapshot and the models it cannot price. |
| `config` | Validate or edit the config file. |

`tokenmtr <command> --help` lists each command's options. `status`, `recap`, `alerts`, `backup`, `doctor`, `quota`, `fx`, `pricing` and `reprice` take `--json`; only `status` and `recap` have a stable format ([docs/cli-json.md](docs/cli-json.md)).

## Docs

| Page | Content |
| --- | --- |
| [docs/install.md](docs/install.md) | Install, first run, uninstall. |
| [docs/config.md](docs/config.md) | Every `config.toml` key. |
| [docs/cli-json.md](docs/cli-json.md) | Stable JSON output of `status` and `recap`. |
| [docs/status.schema.json](docs/status.schema.json) | JSON Schema of `tokenmtr status --json` and `recap --json`. |
| [docs/export-schema.md](docs/export-schema.md) | Columns shared by every export sink. |
| [docs/control-protocol.md](docs/control-protocol.md) | How the CLI and the app talk to the daemon, and how the daemon watches files and shuts down. |
| [docs/adding-a-sink.md](docs/adding-a-sink.md) | Steps to add an export sink. |
| [docs/adding-a-platform.md](docs/adding-a-platform.md) | Steps to port TokenMtr to another OS. |
| [docs/release.md](docs/release.md) | Build, sign and ship a release. |

## Development

```sh
just check   # fmt, clippy, tests, then UI typecheck, lint and tests; run before finishing a change
just dev     # desktop app with hot reload
just goldens # regenerate the UI's golden fixtures (apps/desktop/src/shared/schemas/__fixtures__) from the Rust types (UPDATE_GOLDENS=1)
```

- Homebrew `cargo` ignores `rust-toolchain.toml` and lacks the `x86_64` standard library; the workspace needs Rust 1.95 or newer, so put rustup's `~/.cargo/bin` first in `PATH`.
- On a fresh clone, run `pnpm -C apps/desktop install && pnpm -C apps/desktop build` before `just check`: the desktop crate embeds the git-ignored `apps/desktop/dist`.
- `apps/desktop/src-tauri` is outside the workspace's default members; build it with `cargo build -p tokenmtr-desktop`.
- Test timestamps with explicit time zones: day buckets use the configured IANA zone.
- `just bench` includes the store and query benchmarks (`crates/tokenmtr-query/benches/queries.rs`): they build a store of 1,000,000 events first (about a minute, ~700 MB in a temp dir). Set `TOKENMTR_BENCH_STORE=<file>` to keep and reuse it, `TOKENMTR_BENCH_EVENTS` to change its size; run long groups with `--sample-count 10`.
- UI work starts from the briefs in [apps/desktop/src/shared/BRIEF.md](apps/desktop/src/shared/BRIEF.md).
- Release builds and their gotchas: [docs/release.md](docs/release.md).

## Architecture

A Rust workspace. The daemon (`tokenmtr-daemon`) scans the agents' session files through per-agent parsers (`tokenmtr-agents`) into a local SQLite store (`tokenmtr-store`). `tokenmtr-query` turns the store into every figure shown, `tokenmtr-export` pumps rows to the sinks, `tokenmtr-report` renders reports, and `tokenmtr-remote` fetches quota, FX rates and prices. The CLI (`apps/cli`) and the Tauri menu-bar app (`apps/desktop`, a React UI over Tauri commands) share `tokenmtr-app`, read the store directly and control the daemon over a local socket ([docs/control-protocol.md](docs/control-protocol.md)). OS services (paths, background service, control transport, secrets) sit behind one trait in `tokenmtr-platform`. Product names live in `tokenmtr-brand`.

## License

Apache-2.0, see [LICENSE](LICENSE).
