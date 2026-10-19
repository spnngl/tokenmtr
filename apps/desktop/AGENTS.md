# apps/desktop (UI)

React UI of the menu-bar app; the Tauri shell is `src-tauri/` (read `src-tauri/AGENTS.md` before touching it).

## Rules

- Write UI per the `ui-expert` skill, starting from the briefs in `src/shared/BRIEF.md`.
- Screens read data only through `DataSource` (`src/shared/data-source.ts`). The Tauri implementation (`src/shared/tauri-data-source.ts`) parses every answer with the Zod schemas in `src/shared/schemas/`; the seeded mock (`src/shared/mock/`) serves tests, screenshots and `pnpm dev` (`?mock=<scenario>`).
- Generated files are rebuilt, not edited: `src/shared/bindings.ts` with `just bindings`, `src/shared/schemas/__fixtures__/` with `just goldens`.

## Checking a UI change

1. Run `just check-ui`.
2. Look at the result: `pnpm exec playwright test` (from this directory) screenshots every screen and state in light and dark from the mock into `e2e/screenshots/<theme>/`. Done when the screens you touched read right at every width shot (1280 and 390 px, the recap at 360 px).

A new screen or state? Add the route to `SCREENS` and `ROUTES`, or the mock scenario to `STATES`, in `e2e/screenshots.spec.ts` so step 2 shoots it.

A Rust response type changed? Follow `crates/tokenmtr-query/AGENTS.md`.
