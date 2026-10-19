# Configuration reference

TokenMtr reads one TOML file, `config.toml`, from the first of:

1. `$TOKENMTR_CONFIG`, which must be an absolute path;
2. `$XDG_CONFIG_HOME/tokenmtr/config.toml`, when `XDG_CONFIG_HOME` is absolute;
3. `~/.config/tokenmtr/config.toml`.

The CLI's `--config <path>` overrides all three; `tokenmtr config path` prints the path in use. The `tokenmtr-config` crate parses and validates the file.

- Every key is optional. An empty file is a valid config that applies all defaults below.
- Unknown keys are errors.
- `tokenmtr config check` prints the parsed config, or the first error as `file:line:column: message`.
- The daemon reloads the file when it changes and keeps the old config when the new one is invalid; `tokenmtr doctor` and the app show the rejected file's error.

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
rings = ["session", "weekly"]  # quota rings: session | weekly | per_model; [] = plain icon
hidden_profiles = []           # profiles drawn without rings
shortcut = "Alt+CommandOrControl+U"  # "" disables
live_window = "10m"            # Live block window

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

[git]
default_remote = "origin"      # its URL keys projects; else origin, else the first remote
# default_branch = "main"      # reserved for branch statistics

[[projects]]                   # first match wins; otherwise git resolution
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
# provider = "bedrock"        # only events served by this provider

[[subscriptions]]
id = "claude-max"
provider = "anthropic"
plan = "max_20x"               # pro | max_5x | max_20x | team | enterprise | custom
agents = ["claude", "pi"]
# profiles = ["default"]       # optional narrowing
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

[integrations.superset]
enabled = true                 # read-only hints for deleted worktrees, when installed
# dir = "~/.superset"

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
# dir = "~/Library/Application Support/dev.tokenmtr.app/exports"  # default
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
dir = "~/Library/CloudStorage/Dropbox/tokenmtr"  # protected folder: grant access first
mode = "append"
redact = { cwd = "hash", git_branch = "drop" }

# [[sinks]]                    # planned, not accepted yet
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

Types: `[x]` is a list of `x`. A default of "platform" means a path under `~/Library/Application Support/dev.tokenmtr.app/` (store `usage.db`, `backups/`, `exports/`).

### Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `schema_version` | integer | `1` | Must equal `1`. |

### `[machine]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | host name | Label of this machine in the UI and exports; rows also carry a stable `machine_id`. A blank value means the default. |

### `[display]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `timezone` | `"system"` or IANA name | `"system"` | Zone that buckets days, weeks and months. |
| `currency` | `"USD"` or `"EUR"` | `"USD"` | Currency shown; the default currency of `[[budgets]]`. |

### `[quota]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Poll Claude's quota windows with the Claude Code sign-in ([install step 6](install.md#6-keychain-access-for-quota)). |
| `interval` | duration | `"5m"` | Poll interval; at least `"1m"`. |

When the account has extra usage switched on (Claude's pay-as-you-go spend beyond the plan), each poll also stores it as an `extra_usage` window: this month's spend against the monthly cap, in USD, with no reset time. The spend added since the previous poll lands on one usage event per profile and local day, with no tokens; a lower spend than the previous poll counts as a new month. The first poll counts the whole month so far on its day and marks it approximate.

### `[fx]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Fetch the ECB EUR/USD reference rate daily. |
| `fixed_eur_per_usd` | money amount | unset | Rate that replaces the ECB entirely; must be positive. |

### `[menubar]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `agents` | `[agent]` | all enabled agents | Agents with a recap tab. Non-empty, unique, enabled. |
| `rings` | `["session" \| "weekly" \| "per_model"]` | `["session", "weekly"]` | Quota windows drawn as rings in the menu bar, one per window and profile: `session` is the 5-hour window, `weekly` the 7-day window, `per_model` every weekly window scoped to one model. Each ring shows the percent used, amber from 70 %, red from 90 %; a dashed ring means the profile's quota cannot be read. Unique; `[]`, or no quota yet, shows the plain icon. The menu bar shows no text. |
| `hidden_profiles` | `[profile]` | `[]` | Profiles drawn without rings. Unique. |
| `shortcut` | string | `"Alt+CommandOrControl+U"` | Global shortcut that shows or hides the recap; `""` disables it. The desktop shell parses the syntax. |
| `live_window` | duration | `"10m"` | Window of the recap's Live block: tokens per minute, API value per hour and active sessions over this window, shown while it has events. |

### `[alerts]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `quota_thresholds` | `[percent]` | `[80, 95]` | Quota utilization levels that raise an alert, once per window and level; for the `extra_usage` window, once per calendar month and level. Stored sorted and unique. |
| `quota_reset` | bool | `true` | Announce quota resets. Extra usage has no reset time and never announces one. |
| `budget_thresholds` | `[percent]` | `[80, 100]` | Budget consumption levels that raise an alert, once per period and level. Stored sorted and unique. |
| `sink_failing_after` | duration | `"30m"` | How long a sink may fail before alerting. |
| `daemon_stale_after` | duration | `"15m"` | Heartbeat age that raises `daemon_stale`. |

### `[advisor]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `headroom` | percent | `80` | Highest scaled utilization a recommended plan may reach. |

### `[backup]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Back up the store: an integrity-checked, zstd-compressed copy `usage-<UTC timestamp>.db.zst`. `tokenmtr backup now` runs one on demand. |
| `interval` | duration | `"1d"` | Time between backups. |
| `keep` | integer | `14` | Backups kept; at least 1. |
| `dir` | path | platform | Backup directory. |

### `[import]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `claude_stats_cache` | bool | `true` | Import Claude's `stats-cache.json` on each scan, for days before the first transcript on disk. |

### `[git]`

How a git repository becomes a project. TokenMtr reads the repository's config and refs, never runs `git` and never fetches.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `default_remote` | string | `"origin"` | The remote whose URL is the project key. A repository without it uses `origin`, else its first remote by name, else its root path. Changing it re-resolves known projects on the next scan. |
| `default_branch` | string | unset | Your work branch, such as `"main"`. Reserved for upcoming branch statistics; nothing uses it yet. |

A remote-keyed project is named `owner/repo` (the URL path after the host, `group/subgroup/repo` for nested groups). Every remote of the repository is kept, as `host/owner/repo` without credentials, with its default branch when `refs/remotes/<remote>/HEAD` exists; the project page lists them.

### `[[projects]]`

Rules are tried in file order; the first match names the project. Without a match, the project is the session's git repository (its remote, see `[git]`, else its root path). A working directory that is gone, or is not a git repository, takes its project from the Superset worktree that was there (see `[integrations.superset]`): the main repository when it still exists, else the repository URL Superset recorded. Otherwise the project is the working directory.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `match` | glob | required | Pattern for the session's working directory. `~` expands; `*` stays inside one directory, `**` crosses directories. |
| `name` | string | required | Project name. |
| `client` | string | unset | Client label; non-blank. |

### `[[budgets]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | id | required | Unique among budgets. |
| `period` | `day`, `week`, `month` | required | Budget period, calendar-aligned in `display.timezone`. |
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
| `pricing.auto_update` | bool | `true` | Refresh the LiteLLM snapshot daily. A refresh prices only new events; `tokenmtr reprice` re-estimates stored ones. |
| `overrides[].model` | string | required | Model id exactly as the agent reports it. |
| `overrides[].input_per_m` | money amount | required | USD per million input tokens. |
| `overrides[].output_per_m` | money amount | required | USD per million output tokens. |
| `overrides[].cache_read_per_m` | money amount | unset | USD per million cache-read tokens. |
| `overrides[].cache_write_5m_per_m` | money amount | unset | USD per million 5-minute cache-write tokens. |
| `overrides[].cache_write_1h_per_m` | money amount | unset | USD per million 1-hour cache-write tokens. |
| `overrides[].provider` | string | unset | Apply only to events served by this provider, as LiteLLM names it (`anthropic`, `bedrock`, `vertex_ai`). An override with a provider beats one without for that provider's events. |

### `[[subscriptions]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | id | required | Unique among subscriptions. |
| `provider` | string | required | Provider whose traffic it covers. |
| `plan` | `pro`, `max_5x`, `max_20x`, `team`, `enterprise`, `custom` | required | Plan. |
| `agents` | `[agent]` | required | Covered agents; non-empty, enabled. |
| `profiles` | `[id]` | all | Narrows to these profiles; declared by one of `agents`. |
| `fee` | money amount | list price | Fee per `period`. Required unless `pricing/plans.toml` lists a price for `plan`, `period` and `fee_currency`, which it does not for `team`, `enterprise` and `custom`. |
| `fee_currency` | `"USD"` or `"EUR"` | `"USD"` | Currency of `fee`. |
| `period` | `monthly`, `yearly` | required | Billing period. |
| `start` | date | required | First covered day. |
| `end` | date | unset | First day no longer covered; after `start`. |

Two subscriptions with the same provider, a shared agent, a shared profile and overlapping dates are an error.

Paid cost is the API cost of events billed as `api` plus each fee prorated per day over its billing period, anchored on `start`, plus Claude's extra usage (pay-as-you-go spend beyond the plan, also on subscription profiles). Extra usage adds nothing to API value, tokens or the model, project and session views; budgets with `measure = "paid"` count it.

Known limit: session files do not record whether a call used an API key or a subscription login. An event bills as `subscription` when a subscription matches its provider, agent, profile and day; otherwise it bills as `api`. Declare only the subscriptions you pay for.

### `[daemon]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `scan_interval` | duration | `"5m"` | Interval of the full sweep, a safety net behind file system events. |
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
| `redact_paths` | bool | `false` | Store a keyed hash (BLAKE3, 16 hex characters, key in the store) instead of each event's `cwd` and `git_branch`, and instead of the cwd and path-derived project key in the project map and parse issues. Projects and clients still resolve from the real cwd, which is never written. Applies to events ingested afterwards: rows already stored keep their paths until you erase the store. Rule changes are not re-resolved for hashed cwds. Agent transcript paths (checkpoints, file issues) stay, and so does the project display name (the directory's name). |

### `[integrations.superset]`

[Superset](https://superset.sh) creates git worktrees for agents and deletes them when a task ends. TokenMtr reads Superset's own database, read-only, to name the project of a session whose worktree is gone, and to give a pi session the worktree's branch.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Read the Superset database when it exists. Nothing happens when Superset is not installed. |
| `dir` | path | `"~/.superset"` | Superset's data directory; every `host/<id>/host.db` under it is read. |

When the database cannot be read as expected (a Superset update changed it), the hints stop and Settings shows one `integration_drift` issue; scans go on without them.

### `[agents.<agent>]`

`<agent>` is `claude` or `pi`. An unknown agent is an unknown key.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Collect this agent. |
| `profiles[].id` | id | required | Unique within the agent. |
| `profiles[].roots` | `[path]` | discovery | Session roots; non-empty. |
| `profiles[].config_dir` | path | discovery | Claude only. Roots, credentials and stats cache come from here. Exclusive with `roots`. |

The `default` pi profile reads `PI_CODING_AGENT_SESSION_DIR` when set; otherwise both the `sessionDir` of pi's `settings.json` and the `sessions` folder, each in pi's agent directory (`PI_CODING_AGENT_DIR`, default `~/.pi/agent`).

Every agent has a `default` profile, added with discovery paths when the file omits it. Profiles of one agent must not use equal or nested paths. Discovery paths of an implicit or path-less profile are not checked.

### `[[sinks]]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | id | required | Unique among sinks. |
| `kind` | `csv`, `jsonl`, `sqlite` | required | Sink kind. `csv` and `jsonl` write files. |
| `interval` | duration | `"5m"` | Minimum time between export runs; a run starts only after a write to one of the sink's datasets. |
| `datasets` | `[dataset]` | all five | Subset of `usage_events`, `tool_calls`, `quota_snapshots`, `fx_rates`, `subscriptions`; non-empty, unique. |
| `redact` | table of column to mode | none | Column `cwd`, `project`, `client`, `git_branch`, `account` or `session_id`; mode `hash` (BLAKE3 keyed with a per-install secret, 16 hex characters, so grouping still works) or `drop` (empty CSV cell, JSON `null`, SQL `NULL`). |
| `dir` | path | platform | `csv`, `jsonl` only: output directory. |
| `mode` | `append`, `snapshot` | `"append"` | `csv`, `jsonl` only. `append` adds changed rows with their `revision` (keep the highest per id); `snapshot` atomically rewrites each touched partition with one row per id. |
| `partition` | `month`, `day`, `all` | `"month"` | `csv`, `jsonl` only. Files are named `<dataset>-<partition>.<ext>`, partitioned by row date, e.g. `usage_events-2026-05.csv`. |
| `path` | path | platform | `sqlite` only: database file. |

A key that does not apply to the sink's `kind` is an error. Two sinks that would write the same files are an error: same `kind`, same `dir` or `path`, and a dataset in common. An unset `dir` or `path` means the exports directory, so two such sinks of one kind clash; give one of them its own `dir` or `path`, or disjoint `datasets`.

## Editing the file

`ConfigDocument` appends a `[[subscriptions]]` or `[[projects]]` table, or sets `[menubar]` rings or `[git]` defaults, and leaves every other byte, comments included, untouched. A new project rule goes last, so it has the lowest precedence. An edit is kept only when the new text still validates.
