# CLI JSON output

`tokenmtr status --json` and `tokenmtr recap --json` are the machine interface for scripts, status bars and other apps. Both read the store read-only and work while the daemon is down.

## Stability

`status_schema` is `1`. Within a schema version:

- Fields are only added. None is renamed, retyped or removed.
- Parse unknown fields by ignoring them.
- Enum values (`level`, `state`, `period`, `eta.kind`, …) are only added to; treat an unknown value as the mildest known one.

A breaking change bumps `status_schema` and ships with a new `docs/status.schema.json`. The JSON Schema in [`status.schema.json`](status.schema.json) describes `status --json` exactly; a test in `apps/cli/tests/schema.rs` validates real output against it.

Other `--json` outputs (`alerts`, `quota`, `fx`, `pricing`, `backup`, `doctor`) are not versioned and may change between releases.

## Conventions

| Kind | Format |
| --- | --- |
| Money | Integer micros (1 000 000 = 1.00) in the `currency` of the enclosing object. `api_value_usd_micros` and `api_value_per_hour_usd_micros` are always USD. |
| Timestamps | RFC 3339 UTC, `2026-05-03T14:11:59.865106Z`. |
| Dates | Local days (`display.timezone`), `YYYY-MM-DD`. |
| Absent values | `null`; the key is present. |

## `status --json`

`tokenmtr status [--period today|week|month] --json` prints one object:

| Field | Type | Meaning |
| --- | --- | --- |
| `status_schema` | `1` | Schema version. |
| `generated_at` | timestamp | When the document was built. |
| `system.level` | `ok` \| `warning` \| `error` | Worst of daemon and sinks. A never-started daemon and disabled sinks count as `ok`. |
| `system.daemon.state` | `running` \| `stale` \| `never_started` | From the daemon's heartbeat. |
| `system.daemon.last_heartbeat`, `age_secs` | timestamp, integer or `null` | Latest heartbeat and its age. |
| `system.sinks[]` | object | One entry per configured sink (below). |
| `agents[]` | recap | One recap per agent in `menubar.agents`. |

A sink entry:

| Field | Type | Meaning |
| --- | --- | --- |
| `sink` | string | Sink id. |
| `state` | `healthy` \| `lagging` \| `failing` \| `disabled` | |
| `since`, `last_error` | timestamp, string | Present only when `state` is `failing`. |
| `lag_rows`, `lag_secs` | integer, integer or `null` | Rows not yet exported and the age of the oldest. |
| `last_success_at` | timestamp or `null` | |
| `recent_runs[]` | object | Newest first, at most 10: `dataset`, `started_at`, `finished_at`, `rows`, `error`. |

A recap (also the body of `recap --json`):

| Field | Type | Meaning |
| --- | --- | --- |
| `agent` | string or `null` | `null` for all agents summed. |
| `period` | `today` \| `week` \| `month` | Weeks start on Monday. |
| `range.start`, `range.end` | dates | `end` is exclusive. |
| `currency`, `currency_fallback` | `USD` \| `EUR`, boolean | `currency_fallback` is true when EUR was configured but no FX rate exists yet; amounts are then USD. |
| `totals` | object | Tokens, money and data quality of the period: `events`, `input_uncached`, `cache_read`, `cache_write_5m`, `cache_write_1h`, `output`, `reasoning`, `total_tokens`, `api_value_micros`, `paid_micros`, `subscription_value_micros`, `subscription_fee_micros`, `reported_events`, `estimated_events`, `unpriced_events`, `approximate`, `cached_input_percent`, `cache_savings_micros`, `roi`, `sessions`. |
| `projected_paid_micros` | integer | Paid cost projected to the end of the period. |
| `quota[]` | object | Latest poll per profile: `profile`, `status`, `account`, `age_secs`, `windows[]` with `window`, `used_percent`, `resets_at`, `resets_in_secs`, `eta`. |
| `budgets[]` | object | `id`, `period`, `measure`, `limit_micros`, `spent_micros`, `percent`, `projected_micros`, `projected_percent`, `period_start`, `period_end`. |
| `sparkline[]` | object | API value of the last seven days: `date`, `api_value_micros`. |
| `live` | object or `null` | `tokens_per_minute`, `api_value_per_hour_usd_micros`, `active_sessions`; `null` when the recent window has no events. |
| `top_model` | string or `null` | Model with the biggest API value. |
| `last_activity` | timestamp or `null` | Latest event ever recorded. |

`api_value_micros` is what the usage would cost at API prices; `paid_micros` is what you pay: API-billed usage plus the prorated subscription fees (D28).

`quota[].windows[].eta` is `{"kind": "will_hit", "at": <timestamp>}`, `{"kind": "wont_hit"}` or `{"kind": "not_enough_data"}`.

### Example

```json
{
  "status_schema": 1,
  "generated_at": "2026-10-18T14:11:59.865106Z",
  "system": {
    "level": "ok",
    "daemon": { "state": "running", "last_heartbeat": "2026-10-18T14:11:50Z", "age_secs": 9 },
    "sinks": []
  },
  "agents": [
    {
      "agent": "claude",
      "period": "today",
      "range": { "start": "2026-10-18", "end": "2026-10-19" },
      "currency": "USD",
      "currency_fallback": false,
      "totals": {
        "currency": "USD",
        "currency_fallback": false,
        "events": 1,
        "total_tokens": 1000,
        "api_value_micros": 2500000,
        "paid_micros": 645161,
        "roi": 3.875,
        "sessions": 1
      },
      "projected_paid_micros": 645161,
      "quota": [
        {
          "agent": "claude",
          "profile": "default",
          "fetched_at": "2026-10-18T14:11:59Z",
          "age_secs": 0,
          "status": "ok",
          "account": "me@example.com",
          "windows": [
            {
              "window": "five_hour",
              "used_percent": 42.0,
              "resets_at": "2026-10-18T16:11:59Z",
              "resets_in_secs": 7200,
              "eta": { "kind": "not_enough_data" }
            }
          ]
        }
      ],
      "budgets": [],
      "sparkline": [{ "date": "2026-10-18", "api_value_micros": 2500000 }],
      "live": { "tokens_per_minute": 100, "api_value_per_hour_usd_micros": 15000000, "active_sessions": 1 },
      "top_model": "claude-sonnet",
      "last_activity": "2026-10-18T14:11:59Z"
    }
  ]
}
```

The example omits `totals` fields for brevity; real output carries all of them.

## `recap --json`

`tokenmtr recap [--agent <id>] [--period today|week|month] --json` prints `status_schema`, `generated_at` and `system` as above, followed by the recap fields flattened into the same object. Without `--agent` it sums all agents (`agent` is `null`).

```json
{
  "status_schema": 1,
  "generated_at": "2026-10-18T14:11:59.892069Z",
  "system": { "level": "ok", "daemon": { "state": "never_started", "last_heartbeat": null, "age_secs": null }, "sinks": [] },
  "agent": "claude",
  "period": "today",
  "range": { "start": "2026-10-18", "end": "2026-10-19" },
  "currency": "USD",
  "currency_fallback": false,
  "totals": { "api_value_micros": 2500000, "paid_micros": 645161 },
  "projected_paid_micros": 645161,
  "quota": [],
  "budgets": [],
  "sparkline": [],
  "live": null,
  "top_model": "claude-sonnet",
  "last_activity": "2026-10-18T14:11:59Z"
}
```

## Exit codes

`0` success, `1` failure (message on stderr), `2` usage error. A store written by a newer TokenMtr than the running binary fails with exit code `1` and a message naming both schema versions. `doctor` prints its full report, then exits `1` when `config_error` or `store_error` is set.
