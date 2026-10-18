# TokenMtr

Read `PLAN.md` before any task: it holds every decision (D1–D58), the phase order, and each phase's "Done when". `PLAN.md` is git-ignored and local. Update it in the same change whenever the code deviates from it.

## Rules

- Write Rust per the `rs-coding` skill and TypeScript per the `ts-coding` skill.
- Write UI per the `ui-expert` skill, starting from a brief block.
- Write any agent-facing document (this file, skills, docs reached by a pointer) per the `writing-for-agents` skill.
- Keep one source of truth per fact: version in the workspace `Cargo.toml`, brand strings in `tokenmtr-core::brand`, pricing in `pricing/`.
- Finish a change when `just check` passes.
- Commit per phase using the `git-commit` skill; the human pushes.

## Gotchas

- Homebrew cargo ignores `rust-toolchain.toml`; the workspace targets Rust 1.95+.
- `apps/desktop/src-tauri` is outside `default-members`; build it with `-p tokenmtr-desktop`.
- Test timestamps with explicit time zones: day buckets use the configured IANA zone (D8).
