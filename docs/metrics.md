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
| `tokenmtr_account_info` | gauge | `account`, `agent`, `plan`, `profile`, `rate_limit_tier` | the `account` rows the agents report | Constant 1; the labels carry the plan and rate-limit tier the agent reports. |
| `tokenmtr_build_info` | gauge | `version` | the workspace version | Constant 1; the label carries the TokenMtr version. |
| `tokenmtr_events_total` | counter | `account`, `agent`, `billing`, `client`, `kind`, `model`, `profile`, `project`, `provider`, `service_tier`, `sidechain` | `usage_event` rows, all history | Usage events recorded, by kind. |
| `tokenmtr_last_event_timestamp_seconds` | gauge | `agent`, `profile` | `usage_event` rows, all history, per profile | When the newest usage event happened, in Unix seconds. |
| `tokenmtr_machine_info` | gauge | `arch`, `kernel_version`, `machine_id`, `machine_name`, `os`, `os_build`, `os_id`, `os_name`, `os_version` | the store's machine id, the machine name and the OS | Constant 1; the labels carry the machine and its operating system. |
| `tokenmtr_parse_issues` | gauge | `agent`, `kind` | unresolved `parse_issue` rows | Open parse issues: agent files TokenMtr could not read. |
| `tokenmtr_quota_fetched_at_timestamp_seconds` | gauge | `account`, `agent`, `profile`, `window` | the newest successful quota poll of each profile | When the quota poll behind the quota gauges ran, in Unix seconds. |
| `tokenmtr_quota_resets_at_timestamp_seconds` | gauge | `account`, `agent`, `profile`, `window` | the newest successful quota poll of each profile | When the quota window resets, in Unix seconds. |
| `tokenmtr_quota_used_ratio` | gauge | `account`, `agent`, `profile`, `window` | the newest successful quota poll of each profile | Share of the quota window used, from 0 to 1. |
| `tokenmtr_reasoning_tokens_total` | counter | `account`, `agent`, `billing`, `client`, `model`, `profile`, `project`, `provider`, `service_tier`, `sidechain` | `usage_event` rows, all history | Reasoning tokens, a part of the output tokens. |
| `tokenmtr_series` | gauge |  | this output | Series in this output, this one included. |
| `tokenmtr_session_events_total` | counter | `billing`, `client`, `git_branch`, `kind`, `model`, `project`, `provider`, `service_tier`, `session_id`, `sidechain` | sessions with an event inside `[metrics].session_window` | Usage events of a session active in the session window. |
| `tokenmtr_session_info` | gauge | `account`, `agent`, `machine_id`, `profile`, `session_id` | sessions with an event inside `[metrics].session_window` | Constant 1; links an active session to its agent, profile, account and machine. |
| `tokenmtr_session_last_event_timestamp_seconds` | gauge | `session_id` | sessions with an event inside `[metrics].session_window` | When the newest usage event of an active session happened, in Unix seconds. |
| `tokenmtr_session_reasoning_tokens_total` | counter | `billing`, `client`, `git_branch`, `model`, `project`, `provider`, `service_tier`, `session_id`, `sidechain` | sessions with an event inside `[metrics].session_window` | Reasoning tokens of a session active in the session window. |
| `tokenmtr_session_signals_total` | counter | `kind`, `session_id` | sessions with an event inside `[metrics].session_window` | API errors and context compactions of a session active in the session window. |
| `tokenmtr_session_skill_uses_total` | counter | `session_id`, `skill` | sessions with an event inside `[metrics].session_window` | Skills loaded by a session active in the session window. |
| `tokenmtr_session_start_timestamp_seconds` | gauge | `session_id` | sessions with an event inside `[metrics].session_window` | When the first usage event of an active session happened, in Unix seconds. |
| `tokenmtr_session_tokens_total` | counter | `billing`, `client`, `git_branch`, `model`, `project`, `provider`, `service_tier`, `session_id`, `sidechain`, `type` | sessions with an event inside `[metrics].session_window` | Tokens of a session active in the session window, by type. |
| `tokenmtr_session_tool_calls_total` | counter | `error`, `mcp_server`, `session_id`, `tool` | sessions with an event inside `[metrics].session_window` | Tool calls of a session active in the session window. |
| `tokenmtr_signals_total` | counter | `agent`, `kind`, `profile` | `session_signal` rows, all history | API errors and context compactions, by kind. |
| `tokenmtr_skill_uses_total` | counter | `agent`, `profile`, `skill` | `tool_call` rows that loaded a skill, all history | Skills loaded, by skill. |
| `tokenmtr_snapshot_timestamp_seconds` | gauge |  | this output | When this snapshot of the store was computed, in Unix seconds. |
| `tokenmtr_tokens_total` | counter | `account`, `agent`, `billing`, `client`, `model`, `profile`, `project`, `provider`, `service_tier`, `sidechain`, `type` | `usage_event` rows, all history | Tokens used, by type. |
| `tokenmtr_tool_calls_total` | counter | `agent`, `error`, `mcp_server`, `profile`, `tool` | `tool_call` rows, all history | Tool calls, by tool. |
<!-- metrics:end -->

`billing` is `subscription` when a configured subscription covers the profile on the event's local
day (the same rule as the CLI's reports), else `api`. `project` is the normalised git remote
(`github.com/acme/app`), else the project name, else the last segment of the project key. An event
with no project has an empty `project`. `account` is the account the agent reports (email, else
organization; on quota gauges, else the account the poll recorded), else the configured `account`
(the profile's, else the agent's, else the top-level key in `docs/config.md`), else empty.
`service_tier` is the tier the agent reports (for example `priority` or `flex`), else empty.
`sidechain` is `true` for an event the agent marks as a sidechain (a subagent's call), else `false`.

`machine_id` is the store's id, stable for the life of the store. `tokenmtr_machine_info` links it to
the machine name and the operating system, read once on first use. On Linux, `os_id`,
`os_version`, `os_name` and `os_build` are `ID`, `VERSION_ID`, `PRETTY_NAME` and `BUILD_ID` of
`/etc/os-release` (else `/usr/lib/os-release`), and `kernel_version` is the kernel release. On
macOS `os_id` is `macos`; the others come from `SystemVersion.plist` (`os_build` is the build,
such as `24F74`) and `uname -r`. `os` and `arch` are those of the TokenMtr build. The TokenMtr version is on
`tokenmtr_build_info`.

A skill is counted when an agent loads it: Claude Code through its `Skill` tool, pi when it reads a
`SKILL.md` from its first line under a `skills` directory (`~/.pi/agent/skills/<name>/SKILL.md`,
`.agents/skills/<name>/SKILL.md`). Reading a skill to edit it counts too. Slash commands are not
skills. Only tool calls ingested by this version carry a skill.

Tool calls take the working directory and project of the usage event they belong to, or of the
earliest event of their session that has one.

## Total and session families

A total family has no `session_id`, so its series live as long as the project does. Use
`increase()` on it, which counts sessions that start and end between two scrapes:

```promql
sum by (project) (increase(tokenmtr_tokens_total[1d]))
```

A `tokenmtr_session_*` family exists only for sessions with an event inside
`[metrics].session_window` (default `2d`). An expired session's series disappears, and Prometheus
marks it stale. It carries `session_id` (and `git_branch` on usage families) but not `agent`,
`profile` or `account`: `tokenmtr_session_info` links each `session_id` to those and to the
`machine_id`. Read one session with:

```promql
max by (session_id) (last_over_time(tokenmtr_session_tokens_total[2d]))
```

Join a session family to its agent and account with:

```promql
sum by (session_id) (tokenmtr_session_tokens_total)
  * on (session_id) group_left (agent, profile, account) tokenmtr_session_info
```

A session's duration is
`tokenmtr_session_last_event_timestamp_seconds - tokenmtr_session_start_timestamp_seconds`.

Never wrap `increase()` around a `sum` of a session family: the sum drops when a session expires.

## First samples

A counter series that is new to Prometheus loses its first value to `increase()`.

- **Remote write** avoids this: a series that appears while the daemon runs is pushed with a `0`
  sample one millisecond before its first value. The first push after a daemon start or a config
  reload adds none, because the receiver may already hold those series.
- **Scrape** with the plain text format (`0.0.4`) cannot. Ask for OpenMetrics (Prometheus does by
  default when it supports it) and start Prometheus with
  `--enable-feature=created-timestamp-zero-ingestion`: the exporter then sends `_created`
  samples that Prometheus turns into the zero sample. The flag is experimental and its name has
  changed between Prometheus releases; check the feature list of your version.

## Freshness and limits

- The exporter serves the last snapshot. A write to the store refreshes it within
  `[metrics].refresh_interval` (default `30s`); an idle daemon refreshes nothing, except at the
  moment a session leaves the session window. `tokenmtr_snapshot_timestamp_seconds` says when the
  snapshot was computed.
- Counters are totals over the whole store, so these changes move or lower a series, which
  Prometheus reads as a counter reset: late ingest of old logs (a first run, a backfill), a project
  that is re-resolved to another remote, a change to `[[subscriptions]]` or `display.timezone`
  (`billing` changes), a change to an output's exclusions or redaction, and an account change (detected, or the
  `account` keys of the config).
- Metrics stop when the daemon stops. A scrape target goes down (`up == 0`); remote-write series
  go stale after five minutes. Alert on a silent machine with
  `absent_over_time(tokenmtr_build_info{machine="my-mac"}[10m])`.

## Exclusions

Each output has its own exclusions, so a local dashboard can show everything while a hosted one
shows less.

- `exclude_projects` is a list of globs matched against the project key, the project name and the
  normalised git remote. `"github.com/acme/*"` drops every repository of `acme`.
- `exclude_paths` is a list of globs matched against the working directory; `~` expands.
- `*` stops at `/`; use `**` to cross directories. A path alone matches only that directory:
  `~/work/secret` does not leave out `~/work/secret/app`, `~/work/secret/**` does. Matching is
  case-sensitive.
- An event with no working directory (for `exclude_paths`) or no project (for `exclude_projects`)
  is never excluded. A tool call follows its event, or the earliest event of its session that has
  a working directory or a project.
- Excluded events are left out of every family of that output, so a newly excluded series
  disappears, which Prometheus treats as stale. Quota gauges belong to a profile, not a project,
  and stay.
- `exclude_paths` cannot be combined with `[privacy] redact_paths = true`: the stored paths are
  hashes, so a path glob could never match. Use `exclude_projects`.

## Redaction

`redact = { label = "hash" | "drop" }` rewrites a label of that output: `hash` is the keyed hash
that sinks use (stable on this machine, different on another one, not reversible without the
key), `drop` leaves the label empty and merges the series it makes equal. Labels that can be
redacted: `project`, `client`, `git_branch`, `account`, `session_id`, `skill`. `profile`, `model`,
`provider`, `tool` and `mcp_server` cannot be redacted: exclude the project instead when an MCP
server name is private. The default `machine` external label is the host name; set
`external_labels` to send something else. Before pushing to a hosted
service, hash or drop `account` and `session_id` and read the label values once:
`curl localhost:9464/metrics` shows exactly what leaves the machine.

## Secrets

An `auth` table takes its secret from one of three keys; exactly one is allowed:

| Key | Source |
| --- | --- |
| `password` / `token` | The config file itself. Protect the file. |
| `password_env` / `token_env` | An environment variable, read when the output starts. |
| `password_file` / `token_file` | A file, read when the output starts. One trailing newline is removed. A file readable by group or others is refused: `chmod 600` it. |

The daemon starts from launchd, which does not pass your shell's environment. Prefer `*_file`.
To use `*_env`, run `launchctl setenv NAME value` and restart the daemon; the setting does not
survive a reboot. A secret that cannot be read fails that output (`failing` in health, a
`metrics_failing` alert after the grace period) and leaves the other outputs running. A secret is
never printed or logged. A secret is read when the output starts, so a rotated token is picked up
after a daemon restart or a reload that changes the output, not before. An exporter that listens beyond loopback without `auth` logs a warning.

## Scraping

```yaml
scrape_configs:
  - job_name: tokenmtr
    scrape_interval: 30s
    static_configs:
      - targets: ["127.0.0.1:9464"]
    # authorization:                 # when the exporter has auth = { kind = "bearer", ... }
    #   credentials_file: /etc/prometheus/tokenmtr.token
```

The exporter answers `GET /metrics` only (404 elsewhere), negotiates OpenMetrics from `Accept`,
and gzips when the client accepts it. It speaks plain HTTP: put a reverse proxy in front of it to
add TLS. The exporter keeps trying to bind its port (1 s, doubling to 5 min) if another process
holds it.

## Remote write

```toml
[[metrics.remote_write]]
id = "grafana"
url = "https://prometheus-prod-01-eu-west-0.grafana.net/api/prom/push"
auth = { kind = "basic", username = "123456", password_file = "~/.config/tokenmtr/grafana.secret" }
redact = { session_id = "hash", account = "drop" }
```

For Grafana Cloud the username is the numeric instance ID and the password an access policy token
with the `metrics:write` scope. The URL is `https`, or plain HTTP on this machine for a local
receiver; credentials in the URL are refused. TokenMtr follows no redirect and sends only to the
URL you wrote.

Each push is a Prometheus Remote Write 1.0 request (snappy-compressed protobuf, at most 2000
series per request) stamped with the current time and the labels of `external_labels` (default
`machine`). A `5xx`, `408` or `429` answer and a network error are retried with back-off; any
other `4xx` drops that request, which is recorded as the output's last error.

## Health

Every bind and every push is recorded. `tokenmtr status` prints `metrics <id>: <state>` and
`tokenmtr status --json` carries the history under `system.metrics`; the app's status dot turns
to warning while an output fails. After `[alerts].sink_failing_after` (default `30m`) of
continuous failure the daemon raises one `metrics_failing` alert for that output.
