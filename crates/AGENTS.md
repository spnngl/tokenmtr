# Crates

Every rule of the product lives in `crates/`; `apps/` only wire crates to a UI or a command line. Each crate's doc comment (top of its `lib.rs`) states its model: read it before changing the crate.

## Boundaries

- Apps depend on crates; a crate never depends on an app. A Tauri command or a CLI subcommand parses its input, calls into the crates and formats the answer; the logic stays in the crate, so a second front end reuses it.
- `tokenmtr-core` is pure (bytes in, values out). IO lives in the crate that owns it: files in `tokenmtr-agents` and `tokenmtr-store`, network in `tokenmtr-remote`, OS services in `tokenmtr-platform`.
- `tokenmtr-metrics` computes and encodes Prometheus metrics (store in, bytes out) and opens no socket; `tokenmtr-remote` serves and pushes them, and `tokenmtr-daemon` runs them.
- Closed sets are enums whose variants wrap a trait implementation: `AgentKind` (`AgentSource`), `SinkKind` (`Sink`), `ConfigFormat` (`ConfigCodec`), `PlatformKind` (`Platform`). Add a variant and let the compiler's exhaustive matches list the places to update.

## Privacy guardrails

- Store and log metadata only: token counts, models, ids, timestamps, tool names, error flags, skill names. Prompt and response text, tool arguments and results, and error messages read from agent files stay out of the store, logs, parse issues and exports.
- The one exception to tool arguments: the name of the skill a call loads, a `SkillName` (trimmed, 1 to 128 bytes, no control characters). The parsers read it only from Claude Code `Skill` calls (`input.skill`) and pi `read` calls from the first line of a `SKILL.md` under a `skills` directory (the parent directory name of `arguments.path`, never the path; `arguments.offset` says where the read starts). Read no other argument field, and keep invalid names out rather than reporting them.
- Network requests go only to the hosts in `ALLOWED_HOSTS` (`crates/tokenmtr-remote/src/http.rs`) and to the `[[metrics.remote_write]]` URLs the user configures (HTTPS, or loopback over any scheme; `remote_write::Target::new` enforces it), through `tokenmtr-remote`. The scrape endpoint listens only for an exporter the user configures. A new host also changes the privacy sentence at the top of `README.md`.
- Tokens live in `secrecy::SecretString`. Errors and `tracing` carry no headers or bodies; `crates/tokenmtr-remote/tests/secrets.rs` and the e2e scenario check that a fake token appears nowhere.
- Background work raises no OS consent prompt (macOS Keychain, protected folders): secret reads use `SecretStore::get`, and `get_interactive` serves only an explicit user request (`connect_keychain`); cwds under `Paths::protected` folders are skipped lexically, before any filesystem call.

## Branches

- Changing a control-channel message (`crates/tokenmtr-control/src/protocol.rs`): read `docs/control-protocol.md` "Versioning" first, and update its message table.
- Porting to another OS (`tokenmtr-platform`): follow `docs/adding-a-platform.md`.
- Adding a parser of outside data (agent file, HTTP response, config): add a fuzz target per `fuzz/AGENTS.md`.
