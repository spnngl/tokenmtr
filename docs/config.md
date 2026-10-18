# Configuration reference

TokenMtr reads one TOML file, `config.toml` (location: D21 in `PLAN.md`; override with `TOKENMTR_CONFIG`). The `tokenmtr-config` crate parses and validates it.

- Every key is optional. An empty file is a valid config that applies all defaults below.
- Unknown keys are errors.
- `tokenmtr config check` prints the parsed config, or the first error as `file:line:column: message`.
- The daemon reloads the file when it changes and keeps the old config when the new one is invalid (D22).

## Example

The example below shows every table and key; commented keys show optional settings. A test keeps it identical to `crates/tokenmtr-config/tests/fixtures/example.toml`; edit both together.

```toml
schema_version = 1

[machine]
# name = "work-mbp"            # display label; default: host name

[display]
timezone = "system"            # or IANA name, e.g. "Europe/Paris"
currency = "USD"               # USD | EUR

[quota]
enabled = true
interval = "5m"

[fx]
enabled = true                 # ECB fetch
# fixed_eur_per_usd = 0.92     # overrides ECB

[menubar]
agents = ["claude", "pi"]      # one recap tab each; default: all enabled agents
title = "paid_today"           # paid_today | api_today | tightest_quota | none
shortcut = "Alt+CommandOrControl+U"  # "" disables
live_window = "10m"            # Live block window (D58)

[alerts]
quota_thresholds = [80, 95]    # percent
quota_reset = true
budget_thresholds = [80, 100]  # percent
sink_failing_after = "30m"
daemon_stale_after = "15m"

[advisor]
headroom = 80                  # percent

[backup]
enabled = true
interval = "1d"
keep = 14
# dir = "~/Library/Application Support/dev.tokenmtr.app/backups"

[import]
claude_stats_cache = true

[[projects]]                   # first match wins; otherwise git resolution (D31)
match = "~/work/acme-*/**"
name = "acme"
client = "Acme Corp"

[[budgets]]
id = "monthly"
period = "month"               # day | week | month
amount = 300.0
currency = "EUR"
measure = "paid"               # paid | api_value
# agents = ["claude"]; profiles = ["work"]; projects = ["acme"]; clients = ["Acme Corp"]

[pricing]
auto_update = true             # daily LiteLLM refresh

[[pricing.overrides]]          # beats LiteLLM; USD per million tokens
model = "my-private-model"
input_per_m = 3.0
output_per_m = 15.0
# cache_read_per_m, cache_write_5m_per_m, cache_write_1h_per_m: optional

[[subscriptions]]
id = "claude-max"
provider = "anthropic"
plan = "max_20x"               # pro | max_5x | max_20x | team | enterprise | custom
agents = ["claude", "pi"]
# profiles = ["default"]       # optional narrowing (D28)
# fee = 200.0                  # default from pricing/plans.toml
fee_currency = "USD"           # USD | EUR
period = "monthly"             # monthly | yearly
start = "2025-06-01"
# end = "2026-06-01"

[daemon]
scan_interval = "5m"
watch = true
log_level = "info"

[store]
# path = "~/Library/Application Support/dev.tokenmtr.app/usage.db"  # platform default

[logging]
format = "text"                   # text | json

[privacy]
redact_paths = false

[agents.claude]
enabled = true

[[agents.claude.profiles]]
id = "default"
# roots = ["~/.claude/projects"]   # default: platform + env discovery

# [[agents.claude.profiles]]
# id = "work"
# config_dir = "~/.claude-work"    # roots, credentials and stats cache come from here

[agents.pi]
enabled = true

[[agents.pi.profiles]]
id = "default"
# roots = ["~/.pi/agent/sessions"]

[[sinks]]
id = "local-csv"
kind = "csv"
# dir = "~/Library/Application Support/dev.tokenmtr.app/exports"  # default (D21, D50)
mode = "append"                # append | snapshot
partition = "month"            # month | day | all
datasets = ["usage_events", "tool_calls", "quota_snapshots", "fx_rates", "subscriptions"]
interval = "5m"

[[sinks]]
id = "local-sqlite"
kind = "sqlite"
# path = "~/Library/Application Support/dev.tokenmtr.app/exports/export.db"  # default
interval = "5m"

[[sinks]]
id = "shared-jsonl"
kind = "jsonl"
dir = "~/Library/CloudStorage/Dropbox/tokenmtr"  # protected folder: grant access first (D50)
mode = "append"
redact = { cwd = "hash", git_branch = "drop" }   # D40

# [[sinks]]                    # later (Phase 13)
# id = "warehouse"
# kind = "postgres"
# url_env = "TOKENMTR_PG_URL"   # secrets never live in the file
```

## Value formats

| Format | Written as | Notes |
| --- | --- | --- |
| duration | `"<n><unit>"`, unit `s`, `m`, `h` or `d` | `"90s"`, `"5m"`, `"1d"`. Zero is rejected. |
| percent | integer 1 to 100 | |
| money amount | TOML integer or float | `300`, `300.0`, `0.92`. |
| date | quoted string `"YYYY-MM-DD"` | Quotes are required. |
| path | string starting with `/` or `~/` | `~` expands to `$HOME` (`%USERPROFILE%` on Windows). Relative paths are errors. |
| id | 1 to 64 characters from letters, digits, `.`, `_`, `-` | Used by `id` keys and `profiles` lists. |

## Keys

Types: `[x]` is a list of `x`. A default of "platform" means the location from D21.

### Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `schema_version` | integer | `1` | Must equal `1`. |

### `[machine]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | host name | Label of this machine in the UI and exports (D30). A blank value means the default. |

### `[display]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `timezone` | `"system"` or IANA name | `"system"` | Zone that buckets days (D8). |
| `currency` | `"USD"` or `"EUR"` | `"USD"` | Currency shown; the default currency of `[[budgets]]`. |

### `[quota]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Poll provider quota (D26). |
| `interval` | duration | `"5m"` | Poll interval; at least `"1m"`. |

### `[fx]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Fetch ECB rates (D27). |
| `fixed_eur_per_usd` | money amount | unset | Rate that replaces the ECB entirely; must be positive. |

### `[menubar]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `agents` | `[agent]` | all enabled agents | Agents with a recap tab. Non-empty, unique, enabled. |
| `title` | `paid_today`, `api_today`, `tightest_quota`, `none` | `"paid_today"` | Tray title (D29). |
| `shortcut` | string | `"Alt+CommandOrControl+U"` | Global shortcut (D44); `""` disables it. The desktop shell parses the syntax. |
| `live_window` | duration | `"10m"` | Window of the Live block (D58). |

### `[alerts]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `quota_thresholds` | `[percent]` | `[80, 95]` | Quota utilization levels that raise an alert (D36). Stored sorted and unique. |
| `quota_reset` | bool | `true` | Announce quota resets. |
| `budget_thresholds` | `[percent]` | `[80, 100]` | Budget consumption levels that raise an alert (D34). Stored sorted and unique. |
| `sink_failing_after` | duration | `"30m"` | How long a sink may fail before alerting. |
| `daemon_stale_after` | duration | `"15m"` | Heartbeat age that raises `daemon_stale`. |

### `[advisor]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `headroom` | percent | `80` | Highest scaled utilization a recommended plan may reach (D35). |

### `[backup]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Back up the store (D37). |
| `interval` | duration | `"1d"` | Time between backups. |
| `keep` | integer | `14` | Backups kept; at least 1. |
| `dir` | path | platform | Backup directory. |

### `[import]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `claude_stats_cache` | bool | `true` | Import Claude's stats cache on each scan (D38). |

### `[[projects]]`

Rules are tried in file order; the first match names the project (D31).

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `match` | glob | required | Pattern for the session's working directory. `~` expands; `*` stays inside one directory, `**` crosses directories. |
| `name` | string | required | Project name. |
| `client` | string | unset | Client label; non-blank. |

### `[[budgets]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | id | required | Unique among budgets. |
| `period` | `day`, `week`, `month` | required | Budget period (D34). |
| `amount` | money amount | required | Limit; must be positive. |
| `currency` | `"USD"` or `"EUR"` | `display.currency` | Currency of `amount`. |
| `measure` | `paid`, `api_value` | `"paid"` | What the budget counts. |
| `agents` | `[agent]` | all | Count only these agents; enabled. |
| `profiles` | `[id]` | all | Count only these profiles; declared by an agent in scope. |
| `projects` | `[string]` | all | Count only these projects. |
| `clients` | `[string]` | all | Count only these clients. |

Each filter list is non-empty and unique when present.

### `[pricing]` and `[[pricing.overrides]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `pricing.auto_update` | bool | `true` | Refresh the LiteLLM snapshot daily (D11). |
| `overrides[].model` | string | required | Model id exactly as the agent reports it. |
| `overrides[].input_per_m` | money amount | required | USD per million input tokens. |
| `overrides[].output_per_m` | money amount | required | USD per million output tokens. |
| `overrides[].cache_read_per_m` | money amount | unset | USD per million cache-read tokens. |
| `overrides[].cache_write_5m_per_m` | money amount | unset | USD per million 5-minute cache-write tokens. |
| `overrides[].cache_write_1h_per_m` | money amount | unset | USD per million 1-hour cache-write tokens. |

### `[[subscriptions]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | id | required | Unique among subscriptions. |
| `provider` | string | required | Provider whose traffic it covers. |
| `plan` | `pro`, `max_5x`, `max_20x`, `team`, `enterprise`, `custom` | required | Plan. |
| `agents` | `[agent]` | required | Covered agents; non-empty, enabled. |
| `profiles` | `[id]` | all | Narrows to these profiles; declared by one of `agents`. |
| `fee` | money amount | list price | Fee per `period`. Required unless `pricing/plans.toml` lists a price for `plan`, `period` and `fee_currency` (D28). |
| `fee_currency` | `"USD"` or `"EUR"` | `"USD"` | Currency of `fee`. |
| `period` | `monthly`, `yearly` | required | Billing period. |
| `start` | date | required | First covered day. |
| `end` | date | unset | First day no longer covered; after `start`. |

Two subscriptions with the same provider, a shared agent, a shared profile and overlapping dates are an error (D28).

### `[daemon]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `scan_interval` | duration | `"5m"` | Full-sweep interval (D16). |
| `watch` | bool | `true` | Scan on file system events. |
| `log_level` | `error`, `warn`, `info`, `debug`, `trace` | `"info"` | Log verbosity. |

### `[logging]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `format` | `text`, `json` | `"text"` | Line format of the daemon log, file and terminal: `json` writes one JSON object per line. The level is `daemon.log_level`. |

### `[store]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `path` | path | platform | Store file. |

### `[privacy]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `redact_paths` | bool | `false` | Store a keyed hash (BLAKE3, 16 hex characters, key in the store) instead of each event's `cwd` and `git_branch`, and instead of the cwd and path-derived project key in the project map and parse issues (D13). Projects and clients still resolve from the real cwd, which is never written. Applies to events ingested afterwards: rows already stored keep their paths until you erase the store. Rule changes are not re-resolved for hashed cwds. Agent transcript paths (checkpoints, file issues) stay, and so does the project display name (the directory's name). |

### `[agents.<agent>]`

`<agent>` is `claude` or `pi`. An unknown agent is an unknown key.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Collect this agent. |
| `profiles[].id` | id | required | Unique within the agent. |
| `profiles[].roots` | `[path]` | discovery | Session roots; non-empty. |
| `profiles[].config_dir` | path | discovery | Claude only. Roots, credentials and stats cache come from here. Exclusive with `roots`. |

Every agent has a `default` profile, added with discovery paths when the file omits it (D30). Profiles of one agent must not use equal or nested paths. Discovery paths of an implicit or path-less profile are not checked.

### `[[sinks]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | id | required | Unique among sinks. |
| `kind` | `csv`, `jsonl`, `sqlite` | required | Sink kind. `csv` and `jsonl` write files. |
| `interval` | duration | `"5m"` | Minimum time between export runs (D47). |
| `datasets` | `[dataset]` | all five | Subset of `usage_events`, `tool_calls`, `quota_snapshots`, `fx_rates`, `subscriptions`; non-empty, unique. |
| `redact` | table of column to mode | none | Column `cwd`, `project`, `client`, `git_branch`, `account` or `session_id`; mode `hash` or `drop` (D40). |
| `dir` | path | platform | `csv`, `jsonl` only: output directory. |
| `mode` | `append`, `snapshot` | `"append"` | `csv`, `jsonl` only (D18). |
| `partition` | `month`, `day`, `all` | `"month"` | `csv`, `jsonl` only (D18). |
| `path` | path | platform | `sqlite` only: database file. |

A key that does not apply to the sink's `kind` is an error.

## Editing the file

`ConfigDocument` appends a `[[subscriptions]]` or `[[projects]]` table and leaves every other byte, comments included, untouched. A new project rule goes last, so it has the lowest precedence. An edit is kept only when the new text still validates.
