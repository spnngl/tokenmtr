# Metrics outputs

TokenMtr can publish its token counters to Prometheus in two ways, both configured under
`[metrics]` in [config.md](config.md):

- an **exporter** that serves `GET /metrics` for Prometheus to scrape;
- a **remote write** output that pushes to a Prometheus-compatible endpoint (Prometheus, Mimir,
  Grafana Cloud, VictoriaMetrics, Thanos receive).

Both read the local store, never the agents' files. Neither sends prices or money: costs stay in
the CLI, the app and the sinks. Cost is computed with `tokenmtr` pricing, which Prometheus does not
have; multiply the token series by your own rates if you want a cost panel.

## Catalog

Every output serves the same families. Counters are absolute totals over all history; the daemon
recomputes them from the store, so a counter is monotonic unless usage rows are deleted (a
`reprice --all`, a removed agent profile or a pruned history can lower it; Prometheus treats that
as a counter reset).

<!-- metrics:start -->

| Metric | Type | Labels | Source | Meaning |
| --- | --- | --- | --- | --- |
| `tokenmtr_build_info` | gauge | `version` | the workspace version | Constant 1; the label carries the TokenMtr version. |
| `tokenmtr_events_total` | counter | `account`, `agent`, `billing`, `client`, `kind`, `model`, `profile`, `project`, `provider` | `usage_event` rows, all history | Usage events recorded, by kind. |
| `tokenmtr_quota_fetched_at_timestamp_seconds` | gauge | `account`, `agent`, `profile`, `window` | the newest successful quota poll of each profile | When the quota poll behind the quota gauges ran, in Unix seconds. |
| `tokenmtr_quota_resets_at_timestamp_seconds` | gauge | `account`, `agent`, `profile`, `window` | the newest successful quota poll of each profile | When the quota window resets, in Unix seconds. |
| `tokenmtr_quota_used_ratio` | gauge | `account`, `agent`, `profile`, `window` | the newest successful quota poll of each profile | Share of the quota window used, from 0 to 1. |
| `tokenmtr_reasoning_tokens_total` | counter | `account`, `agent`, `billing`, `client`, `model`, `profile`, `project`, `provider` | `usage_event` rows, all history | Reasoning tokens, a part of the output tokens. |
| `tokenmtr_series` | gauge |  | this output | Series in this output, this one included. |
| `tokenmtr_session_events_total` | counter | `account`, `agent`, `billing`, `client`, `git_branch`, `kind`, `model`, `profile`, `project`, `provider`, `session_id` | sessions with an event inside `[metrics].session_window` | Usage events of a session active in the session window. |
| `tokenmtr_session_reasoning_tokens_total` | counter | `account`, `agent`, `billing`, `client`, `git_branch`, `model`, `profile`, `project`, `provider`, `session_id` | sessions with an event inside `[metrics].session_window` | Reasoning tokens of a session active in the session window. |
| `tokenmtr_session_tokens_total` | counter | `account`, `agent`, `billing`, `client`, `git_branch`, `model`, `profile`, `project`, `provider`, `session_id`, `type` | sessions with an event inside `[metrics].session_window` | Tokens of a session active in the session window, by type. |
| `tokenmtr_session_tool_calls_total` | counter | `agent`, `error`, `mcp_server`, `profile`, `session_id`, `tool` | sessions with an event inside `[metrics].session_window` | Tool calls of a session active in the session window. |
| `tokenmtr_snapshot_timestamp_seconds` | gauge |  | this output | When this snapshot of the store was computed, in Unix seconds. |
| `tokenmtr_tokens_total` | counter | `account`, `agent`, `billing`, `client`, `model`, `profile`, `project`, `provider`, `type` | `usage_event` rows, all history | Tokens used, by type. |
| `tokenmtr_tool_calls_total` | counter | `agent`, `error`, `mcp_server`, `profile`, `tool` | `tool_call` rows, all history | Tool calls, by tool. |
<!-- metrics:end -->

`billing` is `subscription` when a configured subscription covers the profile on the event's local
day (the same rule as the CLI's reports), else `api`. `project` is the normalised git remote
(`github.com/acme/app`), else the project name, else the last segment of the project key. An event
with no project has an empty `project`.

Tool calls take the working directory and project of the usage event they belong to, or of the
earliest event of their session that has one.

## Total and session families

A total family has no `session_id`, so its series live as long as the project does. Use
`increase()` on it, which counts sessions that start and end between two scrapes:

```promql
sum by (project) (increase(tokenmtr_tokens_total[1d]))
```

A `tokenmtr_session_*` family adds `session_id` (and `git_branch` where it applies) but only for
sessions with an event inside `[metrics].session_window` (default `2d`). An expired session's
series disappears, and Prometheus marks it stale. Read one session with:

```promql
max by (session_id) (last_over_time(tokenmtr_session_tokens_total[2d]))
```

Never wrap `increase()` around a `sum` of a session family: the sum drops when a session expires.

## First samples

A counter series that is new to Prometheus loses its first value to `increase()`.

- **Remote write** avoids this: a series that appears while the daemon runs is pushed with a `0`
  sample one millisecond before its first value. The first push after a daemon start or a config
  reload adds none, because the receiver may already hold those series.
- **Scrape** with the plain text format (`0.0.4`) cannot. Ask for OpenMetrics (Prometheus does by
  default when it supports it) and start Prometheus with
  `--enable-feature=created-timestamp-zero-ingestion`: the exporter then sends `_created`
  samples that Prometheus turns into the zero sample.
