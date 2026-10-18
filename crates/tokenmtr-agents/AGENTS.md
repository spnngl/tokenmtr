# tokenmtr-agents

Collectors (one module per agent) and the ingest engine (`src/ingest/`). The crate doc in `src/lib.rs` ("Building a collector") covers the parser side; `src/ingest/mod.rs` covers checkpoints and the scan steps.

## Invariants

- A dedup key format is permanent. The store upserts on `(agent, dedup_key)`, and a file re-parsed from zero after a rewrite must land on the same rows; a new format duplicates history.
- Parsers never panic on any input: a bad line becomes a counted, classified line and at most a parse issue, which carries a `KeySkeleton`, never line content.
- Ingest only reads agent files; it never edits them or spawns `git`.

## Adding an agent

1. Collector: `src/<agent>/` per the crate doc, a variant in `AgentKind` (`src/lib.rs`), and an `AgentId` constant listed in `AgentId::KNOWN` (`crates/tokenmtr-core/src/ids.rs`). Done when `cargo check --workspace --all-targets` compiles; its errors list every `AgentKind` match to extend.
2. Config: an `[agents.<agent>]` table (`crates/tokenmtr-config/src/raw.rs`, `build_agent` in `crates/tokenmtr-config/src/build.rs`), wired in `src/ingest/settings.rs`; follow `crates/tokenmtr-config/AGENTS.md`.
3. Tests: sample files under `fixtures/<agent>/` and `tests/<agent>.rs` with insta snapshots, shaped like `tests/claude.rs`. Done when `cargo test -p tokenmtr-agents` passes with reviewed snapshots.
4. Fuzz: a `<agent>_line` target per `fuzz/AGENTS.md`.
5. App: the agent's `capabilities` (`apps/desktop/src-tauri/src/backend/shell.rs`), its resume command (`apps/desktop/src/features/sessions/resume-command.ts`) and the "Supported agents" list in `README.md`.

Done when `just check` and `just fuzz-smoke` pass.
