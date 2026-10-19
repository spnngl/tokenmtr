# Changelog

All notable changes to this project are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

First release candidate: macOS app (universal, Apple Silicon and Intel) plus the `tokenmtr` CLI and background daemon.

### Added

- Collectors for Claude Code and pi transcripts, with incremental checkpoints, streaming and fork deduplication, tool calls, session signals, and Claude's `stats-cache.json` daily totals for days before the first transcript.
- Permanent SQLite store with revisioned rows, integrity-checked zstd backups (`tokenmtr backup`, `tokenmtr restore`) and `tokenmtr erase`.
- Pricing from a vendored LiteLLM snapshot with a daily conditional refresh, config overrides (optionally per provider) and `tokenmtr reprice [--all]`.
- Subscriptions with prorated fees, paid cost vs API value, budgets, quota forecasts, a plan advisor and alerts.
- Claude quota polling per profile, ECB EUR/USD rates and a USD/EUR display currency.
- Claude extra usage (pay-as-you-go spend beyond the plan): a monthly meter with spend and cap on the quota screen, the menu-bar recap and `tokenmtr quota`, threshold alerts once per month, an Extra usage line in billing, and daily `extra_usage` usage events counted in paid cost (never in API value or tokens). `status --json` and `recap --json` gain `extra_usage_micros` and a per-window `spend`.
- Continuous export to CSV, JSONL and SQLite sinks, with optional path redaction; Markdown, CSV and HTML reports.
- Background daemon (`tokenmtr run`, `tokenmtr service install`) with a versioned control protocol, config hot reload and a single-instance lock.
- Desktop app: menu-bar recap, full dashboard (overview, quota, billing, sessions, insights, models, projects, reports, export, settings), Cmd-K palette, onboarding and notifications.
- `--json` output for scripts; `status --json` and `recap --json` are a stable contract checked against `docs/status.schema.json`.
- Deleted Superset worktrees: a session whose working directory is gone (or is not a git repository) takes its project from Superset's database, read-only (`[integrations.superset]`), and keeps it after the worktree is deleted. pi events get a git branch: the directory's current branch for recent events, else the last branch Superset recorded, so Cost per branch covers pi.
- `tokenmtr quota connect` and a **Connect Claude account** button (Quota screen, menu-bar recap): the daemon asks the Keychain once, the user chooses "Always Allow". A refused Keychain shows as quota status `keychain_denied` instead of `signed_out` or `failed`.
- Billing: **Track it** opens a form (plan, fee, currency, period, start) that adds the subscription to the config file. Projects: **Add rule** asks for the project name, prefilled with the repository name.
- pi's `settings.json` `sessionDir` is a default session root.
- Analytics charts, each with a table view: a Sessions **Analysis** tab (cost histogram, cost against duration, cache-hit ratio, tool error rate, token mix, sessions per day, start heatmap, costliest sessions), per-session cost over time with error and compaction markers, token mix and tool calls, and a billing **Paid against API value** panel with its own range (today, 30 days, 90 days, 1 year, filter range), hourly, daily or monthly buckets, paid split, savings, ROI and a per-agent split.
- Per-model analytics, each with a table view: a session's **Usage per model** (API value, or tokens by kind), the Sessions **Analysis** cost histogram stacked by each session's dominant model, and on Insights tool calls, API errors and compactions per model plus a **Cost by model over time** panel. Charts show the top 5 models and fold the rest into Other; a tool call or signal counts under the model its session used last before it.
- Models and Projects charts, each with a table view and following the filters: Models gains cost by model over time, cost share, cost per million tokens, token mix and cache hit rate per model; Projects gains **Cost over time** (the 8 costliest projects plus Other) and **Cost by model per project**; a project's page gains its cost by model, overall and over time.
- Per-model weekly limits: each model-scoped weekly window Anthropic reports (e.g. **Weekly · Fable**) is polled, stored, forecast and alerted on like the others, under the id `weekly_scoped:<model>`; new models appear without an update. `status --json` and `recap --json` gain a per-window `label`.
- Menu-bar rings: the tray icon draws one ring per selected quota window and profile (amber from 70%, red from 90%, dashed when the quota cannot be read), or the plain icon with none. `[menubar] rings` (`session`, `weekly`, `per_model`; default session and weekly) and `hidden_profiles` choose them, also from Settings.
- Quota donuts: the Quota screen, the overview and the menu-bar recap show each window as a donut (used percent, label, reset), grouped per profile.
- The global range filter defaults to the last 30 days (the billing **Paid against API value** panel follows it) and the last chosen range opens the next launch.
- Plan advisor for Team and Enterprise seats: Claude's rate-limit tier sizes the seat's quota (`..._max_5x` as Max 5x, `..._max_20x` as Max 20x) and the declared subscription sets its fee; the panel names the seat and organization and how it was sized. The advisor JSON replaces `not_applicable` with `no_plan` (no plan detected) and `unknown_quota` (plan detected, quota size unknown); a recommendation gains `sizing` and `current_monthly_fee`, a report gains `organization`.
- Git remotes: every remote of a repository is kept (normalized `host/owner/repo`, no credentials, with its default branch when the clone records one) and the project page lists them. `[git] default_remote` (default `origin`) picks the remote whose URL keys projects (else `origin`, else the first remote); changing it, also from Settings, moves projects and their events on the next scan. `[git] default_branch` is stored for upcoming branch statistics. Projects gain a **Group by** toggle: Remote (the project key) or Folder (repository root, else working directory).
- The stale-data banner counts every finished scan, also one that found nothing new. Missing working directories and protected folders show as Info in Settings and no longer mark the totals incomplete. **Open config** creates a commented starter file when none exists.

### Breaking (for stores and exports built from pre-release commits)

- Store schema 3: pi events without a `responseId` get new deduplication keys. Erase stores built before this change (`tokenmtr erase --yes`) and let the daemon rebuild from the agent files; otherwise those events count twice.
- Checkpoints hash the last line with its newline, so every transcript file is read once more after upgrading; deduplication keeps the totals unchanged.
- Export schema 2: SQLite money columns are INTEGER micros (`cost_reported_usd_micros`, `cost_estimated_usd_micros`, `fee_micros`, `eur_per_usd_nanos`), redactable columns are nullable, `usage_events` gains `superseded` and `subscriptions` gains `removed_at_utc`. A SQLite sink refuses a file of schema 1. Delete the old sink outputs together with the store erase above: the erased store has empty sink cursors, so every sink re-exports from the start.
- Store schema 5: `quota_snapshot` gains `spent_usd_micros` and `limit_usd_micros`; the migration runs on open.
- Export schema 3: `quota_snapshots` gains `spent_usd` and `limit_usd` (SQLite: `spent_usd_micros`, `limit_usd_micros`) and `usage_events.kind` gains `extra_usage`. A SQLite sink refuses a file of schema 2: point it at a new `path` under a new `id` (cursors are kept per sink `id`), so it re-exports from the start. A CSV sink appending to an existing `quota_snapshots` file refuses its old header: move that file away. JSONL lines simply gain the two keys.
- Store schema 6: `project_map.source` gains `inferred` and `quota_snapshot.status` gains `keychain_denied`; the migration runs on open. Control protocol 2: an app and a daemon of different versions ask for a restart.
- Store schema 7: new `project_remote` table; repository-keyed projects are renamed from the last URL segment (`images`) to `owner/repo` (`spnngl/images`). The migration runs on open; exports keep their columns (the project key is unchanged).
- `[menubar] title` is gone: the menu bar shows rings instead of a text title. Remove the key, or the config is rejected.
- `tokenmtr erase` deletes the config with `--delete-config` (`--config <path>` is the global flag that picks the file).
