# TokenMtr

Read `PLAN.md` before any task: it holds every decision (D1–D58), the phase order, and each phase's "Done when". `PLAN.md` is git-ignored and local. Update it in the same change whenever the code deviates from it.

## Rules

- Write Rust per the `rs-coding` skill and TypeScript per the `ts-coding` skill.
- Write UI per the `ui-expert` skill, starting from the briefs in `apps/desktop/src/shared/BRIEF.md`.
- Write any agent-facing document (this file, skills, docs reached by a pointer) per the `writing-for-agents` skill.
- Write user-facing docs (`README.md`, `docs/`) without PLAN.md or D-number references: readers do not have PLAN.md, so state the rule itself.
- Keep one source of truth per fact: version in the workspace `Cargo.toml`, product names in the `tokenmtr-brand` crate (the UI reads them from the generated `bindings.ts`), pricing in `pricing/`.
- Finish a change when `just check` passes.
- Commit per phase using the `git-commit` skill; the human pushes.

## Gotchas

Before building or testing, read the Development section of `README.md`: toolchain, first UI build, desktop crate, time zones. Before a release build, read `docs/release.md`.
