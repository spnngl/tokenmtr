# CLI JSON output

`tokenmtr status --json` and `tokenmtr recap --json` are the machine interface for scripts, status bars and other apps. Both read the store read-only and work while the daemon is down.

## Stability

`status_schema` is `1`. Within a schema version:

- Fields are only added. None is renamed, retyped or removed.
- Parse unknown fields by ignoring them.
- Enum values (`level`, `state`, `period`, `eta.kind`, …) are only added to; treat an unknown value as the mildest known one.

A breaking change bumps `status_schema` and ships with a new `docs/status.schema.json`. The JSON Schema in [`status.schema.json`](status.schema.json) is the field list of `status --json` and `recap --json`: every key it names is present, with the type it gives. A test in `apps/cli/tests/schema.rs` validates real output of both commands against it. The tables below explain meanings; the schema's `$defs` list the fields.

Other `--json` outputs (`alerts`, `quota`, `fx`, `pricing`, `backup`, `doctor`, `reprice`) are not versioned and may change between releases.

## Conventions

| Kind | Format |
| --- | --- |
| Money | Integer micros (1 000 000 = 1.00) in the `currency` of the enclosing object. `api_value_usd_micros` and `api_value_per_hour_usd_micros` are always USD. |
| Timestamps | RFC 3339 UTC, `2026-05-03T14:11:59.865106Z`. |
| Dates | Local days (`display.timezone`), `YYYY-MM-DD`. |
| Absent values | `null`; the key is present. One exception: a sink's `since` and `last_error` are omitted unless its `state` is `failing`. |

## `status --json`

`tokenmtr status [--period today|week|month] --json` prints one object:

| Field | Type | Meaning |
| --- | --- | --- |
| `status_schema` | `1` | Schema version. |
| `generated_at` | timestamp | When the document was built. |
| `system.level` | `ok` \| `warning` \| `error` | Worst of daemon, sinks and metrics outputs (a failing output is a `warning`). A never-started daemon and disabled sinks count as `ok`. |
| `system.daemon.state` | `running` \| `stale` \| `never_started` | From the daemon's heartbeat. |
| `system.daemon.last_heartbeat`, `age_secs` | timestamp, integer or `null` | Latest heartbeat and its age. |
| `system.sinks[]` | object | One entry per configured sink (below). |
| `system.metrics[]` | object | One entry per configured metrics output, exporters first (below). Added within schema 1; absent from older documents. |
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

A metrics output entry:

| Field | Type | Meaning |
| --- | --- | --- |
| `output` | string | Output id. |
| `kind` | `exporter` \| `remote_write` | |
| `state` | `healthy` \| `failing` | An output that never ran is `healthy`. |
| `since`, `last_error` | timestamp, string | Present only when `state` is `failing`. |
| `last_success_at` | timestamp or `null` | |
| `recent_runs[]` | object | Newest first, at most 10: `started_at`, `finished_at`, `series` (pushed; `0` for a bind), `error`. |

A recap (also the body of `recap --json`):

| Field | Type | Meaning |
| --- | --- | --- |
| `agent` | string or `null` | `null` for all agents summed. |
| `period` | `today` \| `week` \| `month` | Weeks start on Monday. |
| `range.start`, `range.end` | dates | `end` is exclusive. |
| `currency`, `currency_fallback` | `USD` \| `EUR`, boolean | `currency_fallback` is true when EUR was configured but no FX rate exists yet; amounts are then USD. |
| `totals` | object (`$defs/totals`) | Tokens, money and data quality of the period. It repeats `currency` and `currency_fallback`; `api_value_usd_micros` is the API value in USD whatever the currency. `reasoning` is part of `output`, so `total_tokens` sums `input_uncached`, `cache_read`, `cache_write_5m`, `cache_write_1h` and `output`. |
| `projected_paid_micros` | integer | Paid cost projected to the end of the period. |
| `quota[]` | object (`$defs/meter`) | Latest poll per profile, with its `agent`, `fetched_at` and `status` (`ok`, `signed_out`, `token_expired`, `failed`, `keychain_denied`), and one entry per window with `label`, `used_percent`, `resets_at`, `eta` and `spend`. |
| `budgets[]` | object (`$defs/budget`) | One entry per configured budget in scope, amounts in the budget's own `currency`. |
| `sparkline[]` | object | API value of the last seven days: `date`, `api_value_micros`. |
| `live` | object or `null` | `tokens_per_minute`, `api_value_per_hour_usd_micros`, `active_sessions`; `null` when the recent window has no events. |
| `top_model` | string or `null` | Model with the biggest API value. |
| `last_activity` | timestamp or `null` | Latest event ever recorded. |

`api_value_micros` is what the usage would cost at API prices; `paid_micros` is what you pay: API-billed usage plus the subscription fees prorated per day, plus Claude's extra usage. `extra_usage_micros` is that extra usage alone; it has no tokens and no API value.

`quota[].windows[].eta` is `{"kind": "will_hit", "at": <timestamp>}`, `{"kind": "wont_hit"}` or `{"kind": "not_enough_data"}`.

The window `extra_usage` is Claude's pay-as-you-go spend this month: `used_percent` is spend over the monthly cap, `resets_at` is `null`, and `spend` holds `spent_micros` and `limit_micros` in its own `currency`, with `approximate` for an old FX rate. Every other window has `spend: null`. The window is missing when extra usage is off.

`window` is a stable id: `five_hour`, `seven_day`, `seven_day_<model>`, `extra_usage`, or `weekly_scoped:<model>` for a weekly limit scoped to one model (the model slug follows the provider's name, so new models appear without an update). `label` is its display name, e.g. `5-hour`, `7-day` or `Weekly · Fable`.

### Example

```json
{
  "status_schema": 1,
  "generated_at": "2026-10-18T14:11:59.865106Z",
  "system": {
    "level": "ok",
    "daemon": { "state": "running", "last_heartbeat": "2026-10-18T14:11:50Z", "age_secs": 9 },
    "sinks": [],
    "metrics": []
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
              "label": "5-hour",
              "used_percent": 42.0,
              "resets_at": "2026-10-18T16:11:59Z",
              "resets_in_secs": 7200,
              "eta": { "kind": "not_enough_data" },
              "spend": null
            },
            {
              "window": "extra_usage",
              "label": "Extra usage",
              "used_percent": 24.69,
              "resets_at": null,
              "resets_in_secs": null,
              "eta": { "kind": "not_enough_data" },
              "spend": { "currency": "USD", "spent_micros": 12345000, "limit_micros": 50000000, "approximate": false }
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
  "system": { "level": "ok", "daemon": { "state": "never_started", "last_heartbeat": null, "age_secs": null }, "sinks": [], "metrics": [] },
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
