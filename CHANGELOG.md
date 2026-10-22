# Changelog

All notable changes to this project are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-10-22

Sources: usage from other machines, next to the ones this machine records.

### Added

- `[[sources]]`, the input counterpart of `[[sinks]]`. Kind `sqlite` imports the SQLite exports (`[[sinks]] kind = "sqlite"`) that other TokenMtr machines write, typically synced by a file-sync tool: `include` and `exclude` globs, `interval` (default 5 minutes). Each file is read from a private copy, from where the last pass stopped; a bad file fails alone and shows in the source's run history. Imported rows count in every total and budget, never in exports, metrics or the menu-bar Live block, and are never exported again ([docs/config.md](docs/config.md#sources)).
- `tokenmtr sources` lists sources with their health, `tokenmtr sources import [id]` imports now and `tokenmtr sources forget <id>` deletes a source's imported rows (the source must be disabled or removed, the daemon stopped).
- Export page: a Sources section with each `sqlite` source's health, imported rows, run history and an **Import now** button. Sessions from another machine show their machine (`<source id> · <file stem>`) and offer no resume command.
- `status --json` gains `system.sources`; a failing `sqlite` source is a warning and raises a `source_failing` alert after `[alerts] source_failing_after` (default 30 minutes). Control protocol 4 adds the alert kind and the `import_now` request.
- Store schema 10: imported rows carry their origin; new `import_cursor` and `import_run` tables. The migration runs on open.

### Changed

- **Breaking:** `[integrations.superset]` is replaced by `[[sources]] kind = "superset"`; the old table is rejected with an error naming the new one. Without a `superset` source the implicit one (id `superset`) stays on, so a config that never mentioned Superset keeps working. Several `superset` sources can read different directories; a source whose database changed shape raises its own `integration_drift` issue, named after the source id.
- macOS folder checks cover the folders `sqlite` sources read, as they do sink folders.
## [0.3.0] - 2026-10-21

A daily activity calendar, session and machine metrics, and Claude quota that follows token refreshes.

### Added

- Overview: a GitHub-style **Daily activity** calendar of tokens or started sessions per local day over up to two years, with the current and longest streaks, the busiest day, keyboard navigation and a per-month table. It ignores the range filter and follows the others.

- The metrics `account` label of a profile whose agent reports no account (pi; Claude without an email or organization) comes from the config: top-level `account`, `[agents.<agent>].account` or `profiles[].account`, the most specific winning ([docs/config.md](docs/config.md)).
- **Edit config file** opens the config file (created when missing) in the default text editor, from Settings and the menu-bar recap; the recap also gets a **Quit TokenMtr** button.
- Metrics: `tokenmtr_session_info` links a `session_id` to its agent, profile, account and `machine_id`; `tokenmtr_machine_info` describes the machine (name, OS from `os-release` or `SystemVersion.plist`, kernel). New families: session start and last-event timestamps, skill uses (total and per session), API errors and compactions (total and per session), account plan and rate-limit tier, open parse issues, and the last event per profile. Usage families gain `service_tier` and `sidechain` ([docs/metrics.md](docs/metrics.md)).
- Tool calls record the skill they load (Claude's `Skill` tool, pi reading a `SKILL.md` under a `skills` directory); the `tool_calls` export gains a `skill` column (export schema 4) and the `skill` column and label can be redacted.

### Changed

- Metrics: `tokenmtr_session_*` families no longer carry `agent`, `profile` or `account`; join on `tokenmtr_session_info` instead.
- Config: `external_labels` can no longer use the label names the new metrics carry (`arch`, `kernel_version`, `machine_id`, `machine_name`, `os`, `os_build`, `os_id`, `os_name`, `os_version`, `plan`, `rate_limit_tier`, `service_tier`, `sidechain`, `skill`); such a config fails to load. `[machine].name` is trimmed and rejected when it has a control character or exceeds 128 bytes.
- Once onboarding is done, the app reinstalls a missing background daemon at launch, so it survives `brew upgrade`, which removes it before installing the new app.

### Fixed

- Claude quota uses a token Claude Code refreshed from the next poll on, without a daemon restart: every poll reads the credentials again, and when the Keychain refuses the daemon after Claude Code rewrote its item, the daemon reads it through `/usr/bin/security`. A Claude profile whose quota is not ok shows **Refresh**, which reads the credentials again and polls at once.

## [0.2.0] - 2026-10-20

Linux packages (deb and rpm for amd64 and arm64) join the macOS app.

### Added

- Linux support: XDG data, log and cache directories, a systemd user unit for the daemon (`tokenmtr service install`), a tray icon with a **Show recap** menu item, and `.deb` and `.rpm` packages for Ubuntu 26.04+, Fedora 44+, Debian testing and openSUSE Tumbleweed. Claude quota reads `~/.claude/.credentials.json` only. The app opens its main window when launched by the user and starts in the tray at login (`--background`), on macOS too.
- Releases carry an SPDX SBOM, `SHA256SUMS` signed with keyless cosign (`SHA256SUMS.sigstore.json`) and GitHub build-provenance and SBOM attestations for every package and the dmg ([docs/install.md](docs/install.md#verify-a-download)).

## [0.1.0] - 2026-10-19

First release: macOS app (universal, Apple Silicon and Intel) plus the `tokenmtr` CLI and background daemon.

### Added

- Menu-bar recap sizes itself to its content (up to the screen height) and has its actions and health line in a header; quota donuts of a profile share one row.
- TokenMtr logo: app and menu-bar icons, sidebar and welcome screen, README; sources in `assets/brand/`, icons rebuilt with `just icons`.
- Collectors for Claude Code and pi transcripts, with incremental checkpoints, streaming and fork deduplication, tool calls, session signals, and Claude's `stats-cache.json` daily totals for days before the first transcript.
- Permanent SQLite store with revisioned rows, integrity-checked zstd backups (`tokenmtr backup`, `tokenmtr restore`) and `tokenmtr erase`.
- Pricing from a vendored LiteLLM snapshot with a daily conditional refresh, config overrides (optionally per provider) and `tokenmtr reprice [--all]`.
- Subscriptions with prorated fees, paid cost vs API value, budgets, quota forecasts, a plan advisor and alerts.
- Claude quota polling per profile, ECB EUR/USD rates and a USD/EUR display currency.
- Claude extra usage (pay-as-you-go spend beyond the plan): a monthly meter with spend and cap on the quota screen, the menu-bar recap and `tokenmtr quota`, threshold alerts once per month, an Extra usage line in billing, and daily `extra_usage` usage events counted in paid cost (never in API value or tokens). `status --json` and `recap --json` gain `extra_usage_micros` and a per-window `spend`.
- Continuous export to CSV, JSONL and SQLite sinks, with optional path redaction; Markdown, CSV and HTML reports.
- Prometheus outputs under `[metrics]`: a scrape endpoint (`[[metrics.exporters]]`, `127.0.0.1:9464` by default, plain text or OpenMetrics) and Remote Write 1.0 pushes (`[[metrics.remote_write]]`) of token, event, tool-call and quota series, with no money. Each output has its own project and path exclusions (globs) and label redaction; secrets come from the file, an environment variable or a `chmod 600` file. Output health shows in `status`, and a failing output raises a `metrics_failing` alert (control protocol 3). See `docs/metrics.md`.
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
- Releases: pushing a `v<version>` tag builds, signs and publishes the universal dmg with its public signing certificate and `SHA256SUMS`. Homebrew installs it with `brew install --cask spnngl/tap/tokenmtr`.

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

[Unreleased]: https://github.com/spnngl/tokenmtr/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/spnngl/tokenmtr/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/spnngl/tokenmtr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/spnngl/tokenmtr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/spnngl/tokenmtr/releases/tag/v0.1.0
