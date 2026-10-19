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

### Breaking (for stores and exports built from pre-release commits)

- Store schema 3: pi events without a `responseId` get new deduplication keys. Erase stores built before this change (`tokenmtr erase --yes`) and let the daemon rebuild from the agent files; otherwise those events count twice.
- Checkpoints hash the last line with its newline, so every transcript file is read once more after upgrading; deduplication keeps the totals unchanged.
- Export schema 2: SQLite money columns are INTEGER micros (`cost_reported_usd_micros`, `cost_estimated_usd_micros`, `fee_micros`, `eur_per_usd_nanos`), redactable columns are nullable, `usage_events` gains `superseded` and `subscriptions` gains `removed_at_utc`. A SQLite sink refuses a file of schema 1. Delete the old sink outputs together with the store erase above: the erased store has empty sink cursors, so every sink re-exports from the start.
- Store schema 5: `quota_snapshot` gains `spent_usd_micros` and `limit_usd_micros`; the migration runs on open.
- Export schema 3: `quota_snapshots` gains `spent_usd` and `limit_usd` (SQLite: `spent_usd_micros`, `limit_usd_micros`) and `usage_events.kind` gains `extra_usage`. A SQLite sink refuses a file of schema 2: point it at a new `path` under a new `id` (cursors are kept per sink `id`), so it re-exports from the start. A CSV sink appending to an existing `quota_snapshots` file refuses its old header: move that file away. JSONL lines simply gain the two keys.
- `tokenmtr erase` deletes the config with `--delete-config` (`--config <path>` is the global flag that picks the file).
