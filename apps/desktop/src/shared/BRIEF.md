# UI briefs (ui-expert)

## Full app (`src/app`)

- **User and job**: a developer running Claude Code / pi on subscriptions or API keys. Job: see what the agents cost and how close they are to their limits, then drill into a session, model or project.
- **Lead**: quota proximity and spend vs. value on Overview. Everything else is reached through the sidebar.
- **Primary action**: narrow the view with the filter bar (range, agent, model, project, currency). Filters live in the URL, so a view can be shared and restored.
- **Density**: dense, desktop-first and calm. Mobile-first down to 320px: filters wrap into two columns, the sidebar becomes a drawer below `md`.
- **Mood**: quiet instrument panel. Neutral surfaces, one accent, status colours only for quota thresholds (amber from 70%, red from 90%, always paired with an icon). No decoration.
- **States**: loading, empty, partial, error, stale / daemon offline (`StateView`). Each says what happened and offers the next step.

## Menu-bar recap (`src/recap`, 360px)

- **User and job**: the same developer glancing at the menu bar. Job: answer "am I about to hit a limit, and what did today cost?" in two seconds.
- **Lead**: quota windows with the projected time to 100%.
- **Primary action**: Open dashboard (filtered on the selected agent). Scan now and Settings are secondary.
- **Density**: compact, one column, scrolls inside the footer-anchored window. Blocks an agent cannot report vanish instead of showing zeros.
- **Mood**: same tokens as the app, no chrome of its own.
- **States**: same `StateView` set. Footer status dot (Healthy / Degraded / Offline) is text-labelled, never colour only.

## Tokens and checks

- Semantic tokens live in `styles.css` with light and dark values (dark values are repeated for the OS preference and the explicit `data-theme="dark"`; `light-dark()` is avoided because macOS 13 ships Safari 16).
- Contrast is checked on the tokens with the ui-expert `contrast.ts` script (68/68 pairs pass). Axe in vitest (`axe.ts`) runs with `color-contrast` disabled, since jsdom has no layout or painting.
