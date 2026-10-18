# Export schema

Every sink (CSV, JSONL, SQLite) carries the same five datasets with the same columns and the same
values. The columns are defined once in `tokenmtr_export::schema`; the CSV header, the JSONL keys,
the SQLite DDL and the table below all come from that definition.
`EXPORT_SCHEMA_VERSION` (currently `1`) changes whenever a column is added, removed, renamed or
re-typed. A SQLite sink records it in its `_schema` table and refuses a file of another version.

## Row identity and revisions

- Each dataset has a primary key (first column) and a `revision` (second column).
- A row is re-exported with a higher `revision` whenever any of its values changes.
- The current row of an id is the one with the highest `revision`.
- Delivery is at-least-once: the same `(id, revision)` can arrive twice; sinks drop the repeat.

## Files

File sinks write `<dataset>-<partition>.csv` or `.jsonl` into the sink directory.
The partition comes from the dataset's partition column, in UTC: `2026-05` (`month`, default),
`2026-05-01` (`day`) or `all`. Subscriptions are always one `subscriptions-all` file.

| Mode | Behaviour | Reading the current state |
| --- | --- | --- |
| `append` (default) | Appends each new `(id, revision)` row; a changed row is appended again with its higher revision. A new CSV file starts with the header row. | Keep the row with the highest `revision` per primary key. |
| `snapshot` | Rewrites each touched partition atomically, one row per id, sorted by id. | Read the file as is. |

## Encodings

- CSV: RFC 4180 quoting, header row, empty field for an empty value.
- JSONL: one object per line, keys are the column names, empty values are `null`; decimals are
  JSON numbers, booleans are `true`/`false`.
- SQLite: one table per dataset named like the dataset, primary key on the first column,
  `STRICT`. Decimals are `REAL`, booleans `INTEGER` (`0`/`1`), timestamps and dates `TEXT`.
  A row is replaced only when the incoming `revision` is higher, so the table holds one row per id.
- Timestamps are ISO-8601 UTC (`2026-05-01T10:00:00Z`); dates are `YYYY-MM-DD`.
- Money columns are decimals in USD with 6 decimal places (`fee` is in `fee_currency`);
  `eur_per_usd` has 9.

## Converting to EUR

Usage rows carry USD only. To convert an event to EUR, join `fx_rates` on the event's UTC date:
take the rate with the latest `date` strictly before the event's `at_utc` date, then multiply the
USD amount by its `eur_per_usd` (EUR for 1 USD). Subscription fees are in `fee_currency` already.

## Redaction

A sink can redact these columns: `cwd`, `project`, `client`, `git_branch`, `account`, `session_id`
(`parent_session_id` follows `session_id`). Per column, `redact` is one of:

- `hash`: the value becomes 16 lowercase hex characters, the first 8 bytes of a BLAKE3 hash keyed
  with the store's redaction key. The same value always gives the same hash on one machine;
  another key gives other hashes. Empty values stay empty.
- `drop`: the value is empty. The column stays, so the schema never varies between sinks.

Redaction runs before encoding, so a redacted sink never holds the raw value.

## Datasets

<!-- schema:start -->

### `usage_events`

Primary key: `event_id`. Partitioned by: `at_utc`.

| Column | Type | Empty | Redact | Meaning |
| --- | --- | --- | --- | --- |
| `event_id` | text | no |  | Primary key: 32 hex chars, stable per machine, agent and dedup key. |
| `revision` | integer | no |  | Change counter of the row within its dataset; the highest revision of an id is current. |
| `machine_id` | text | no |  | UUID of the machine that ingested the event. |
| `agent` | text | no |  | Agent id, e.g. `claude` or `pi`. |
| `profile` | text | no |  | Agent profile name. |
| `provider` | text | yes |  | Model provider, when known. |
| `model` | text | no |  | Model id as the agent reported it. |
| `service_tier` | text | yes |  | Provider service tier, when reported. |
| `session_id` | text | no | `session_id` | Agent session id. |
| `parent_session_id` | text | yes | `session_id` | Session that spawned this one (subagents). |
| `project` | text | yes | `project` | Resolved project key. |
| `client` | text | yes | `client` | Client label of the project. |
| `cwd` | text | yes | `cwd` | Working directory of the session. |
| `git_branch` | text | yes | `git_branch` | Git branch at the event. |
| `at_utc` | timestamp (UTC) | no |  | When the event happened; partitions files. |
| `kind` | text | no |  | `message`, `tool_result`, `compaction`, `branch_summary`, `auxiliary` or `daily_aggregate`. |
| `approximate` | boolean | no |  | True when the row is an estimate, e.g. an imported daily aggregate. |
| `input_uncached` | integer | no |  | Input tokens not served from cache. |
| `cache_read` | integer | no |  | Input tokens read from cache. |
| `cache_write_5m` | integer | no |  | Input tokens written to the 5-minute cache. |
| `cache_write_1h` | integer | no |  | Input tokens written to the 1-hour cache. |
| `output` | integer | no |  | Output tokens. |
| `reasoning` | integer | no |  | Reasoning tokens, when reported separately. |
| `total_tokens` | integer | no |  | Sum of all token columns. |
| `cost_reported_usd` | decimal(6) | yes |  | Cost the agent reported, in USD. |
| `cost_estimated_usd` | decimal(6) | yes |  | Pay-as-you-go cost estimated from `pricing_version`, in USD. |
| `cost_source` | text | no |  | Which cost is the API value: `reported`, `estimated` or `unpriced`. |
| `pricing_version` | text | yes |  | Pricing snapshot behind `cost_estimated_usd`. |

### `tool_calls`

Primary key: `tool_call_id`. Partitioned by: `at_utc`.

| Column | Type | Empty | Redact | Meaning |
| --- | --- | --- | --- | --- |
| `tool_call_id` | text | no |  | Primary key: 32 hex chars, stable per machine, agent and tool-use id. |
| `revision` | integer | no |  | Change counter of the row within its dataset; the highest revision of an id is current. |
| `machine_id` | text | no |  | UUID of the machine that ingested the call. |
| `agent` | text | no |  | Agent id. |
| `profile` | text | no |  | Agent profile name. |
| `session_id` | text | no | `session_id` | Agent session id. |
| `event_id` | text | yes |  | `event_id` of the usage event of the same message, when known. |
| `at_utc` | timestamp (UTC) | no |  | When the tool was invoked; partitions files. |
| `tool_name` | text | no |  | Tool name; for MCP tools the part after the server. |
| `mcp_server` | text | yes |  | MCP server name for MCP tools. |
| `is_error` | boolean | yes |  | Whether the result was an error; empty until a result is seen. |

### `subscriptions`

Primary key: `subscription_id`. Partitioned by: nothing (one file).

| Column | Type | Empty | Redact | Meaning |
| --- | --- | --- | --- | --- |
| `subscription_id` | text | no |  | Primary key: the id from config. |
| `revision` | integer | no |  | Change counter of the row within its dataset; the highest revision of an id is current. |
| `machine_id` | text | no |  | UUID of the machine that wrote the row. |
| `provider` | text | no |  | Provider whose traffic the plan covers. |
| `plan` | text | no |  | `pro`, `max_5x`, `max_20x`, `team`, `enterprise` or `custom`. |
| `agents` | text | no |  | Covered agent ids, comma-separated. |
| `profiles` | text | yes |  | Covered profiles, comma-separated; empty means all. |
| `fee` | decimal(6) | no |  | Fee per billing period, in `fee_currency`. |
| `fee_currency` | text | no |  | `USD` or `EUR`. |
| `period` | text | no |  | `monthly` or `yearly`. |
| `start` | date | no |  | First covered day (inclusive). |
| `end` | date | yes |  | First day no longer covered (exclusive); empty while open. |

### `quota_snapshots`

Primary key: `snapshot_id`. Partitioned by: `fetched_at_utc`.

| Column | Type | Empty | Redact | Meaning |
| --- | --- | --- | --- | --- |
| `snapshot_id` | text | no |  | Primary key: 32 hex chars, stable per machine, agent, profile, window and poll time. |
| `revision` | integer | no |  | Change counter of the row within its dataset; the highest revision of an id is current. |
| `machine_id` | text | no |  | UUID of the machine that polled. |
| `agent` | text | no |  | Agent id. |
| `profile` | text | no |  | Agent profile name. |
| `account` | text | yes | `account` | Account email or organization. |
| `window_id` | text | no |  | Quota window id, e.g. `five_hour`; empty when the poll failed. |
| `window_label` | text | yes |  | Human name of the window, e.g. `5-hour`. |
| `used_percent` | real | yes |  | Percent of the window used; may exceed 100. |
| `resets_at_utc` | timestamp (UTC) | yes |  | When the window resets, when the provider says. |
| `fetched_at_utc` | timestamp (UTC) | no |  | When the poll happened; partitions files. |
| `status` | text | no |  | `ok`, `signed_out`, `token_expired` or `failed`. |

### `fx_rates`

Primary key: `date`. Partitioned by: `date`.

| Column | Type | Empty | Redact | Meaning |
| --- | --- | --- | --- | --- |
| `date` | date | no |  | Primary key: publication date of the ECB reference rate; partitions files. |
| `revision` | integer | no |  | Change counter of the row within its dataset; the highest revision of an id is current. |
| `eur_per_usd` | decimal(9) | no |  | EUR for 1 USD. |
| `source` | text | no |  | Rate source: `ecb`. |


<!-- schema:end -->
