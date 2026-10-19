# TokenMtr

Read `PLAN.md` before any task: it holds every numbered decision, the phase order, and each phase's "Done when". `PLAN.md` is git-ignored and local. Update it in the same change whenever the code deviates from it.

## Rules

- Write Rust per the `rs-coding` skill and TypeScript per the `ts-coding` skill.
- Write agent-facing documents (every `AGENTS.md`, skills, docs reached by a pointer) per the `writing-for-agents` skill. Each `AGENTS.md` has a `CLAUDE.md` symlink beside it.
- Keep one source of truth per fact: version in the workspace `Cargo.toml`, product names in the `tokenmtr-brand` crate (the UI reads them from the generated `apps/desktop/src/shared/bindings.ts`), pricing in `pricing/`, logos in `assets/brand/` (app icons are rendered from `assets/brand/app-icon.svg` by `just icons`).
- Finish a change when `just check` passes.
- Commit per phase using the `git-commit` skill; the human pushes.

## Read before you work

- Before building or testing, read the Development section of `README.md`: toolchain, first UI build, desktop crate, time zones.
- Before changing any crate, read `crates/AGENTS.md` (boundaries, privacy guardrails).
- Before changing money, ids or pricing, read `crates/tokenmtr-core/AGENTS.md`.
- Before changing the schema or store writes, read `crates/tokenmtr-store/AGENTS.md`.
- Before adding an agent or changing a parser or ingest, read `crates/tokenmtr-agents/AGENTS.md`.
- Before changing a query result's JSON shape, read `crates/tokenmtr-query/AGENTS.md`.
- Before adding a sink or changing an export column, read `crates/tokenmtr-export/AGENTS.md`.
- Before adding or changing a config key, read `crates/tokenmtr-config/AGENTS.md`.
- Before changing daemon jobs or scheduling, read `crates/tokenmtr-daemon/AGENTS.md`.
- Before UI work, read `apps/desktop/AGENTS.md`.
- Before adding or changing a Tauri command, read `apps/desktop/src-tauri/AGENTS.md`.
- Before adding a fuzz target or changing fixtures, read `fuzz/AGENTS.md`.
- Before editing `README.md` or `docs/`, read `docs/AGENTS.md`.
- Before a release build, read `docs/release.md`.
