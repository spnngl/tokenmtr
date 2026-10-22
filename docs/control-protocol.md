# Control protocol v4

How clients (CLI, menu-bar app) talk to the daemon. Types live in `crates/tokenmtr-control/src/protocol.rs`; that file is the source of truth, this page is the map.

## Transport

- Unix domain socket at `<data_dir>/control.sock`, mode `0600`, parent directory forced to `0700`. The daemon accepts only peers with the socket owner's uid (`getpeereid`).
- Single instance: the daemon holds an exclusive `flock` on `<data_dir>/daemon.lock` for its whole life and takes it before it touches the socket or the store; a second daemon exits with `AlreadyRunning`. `restore`, `erase` and the CLI's direct scan/backup take the same lock, after stopping the service, so they never run beside a daemon.
- One JSON object per line (NDJSON): UTF-8, terminated by `\n`, no embedded newline, at most 1 MiB. A line that breaks framing (too long, not UTF-8) closes the connection; a line that is not a valid request after the handshake gets `error` / `bad_request` and the connection stays open.
- The `type` field names the variant, in snake case. Unknown fields are ignored by readers.

## Handshake

1. The client sends `{"type":"hello","protocol":4,"client":{"name":"…","version":"…","schema":null}}` first. Any other first message, or none within the handshake timeout, ends the connection (`error` / `bad_request` for a wrong first message).
2. Same `protocol`: the daemon answers `welcome` (`protocol`, `daemon_version`, `store_schema`).
3. Different `protocol`: the daemon answers `incompatible` (`daemon_protocol`, `daemon_version`, `store_schema`, `reason`), sends nothing else and closes. A client that reads the store directly compares `store_schema` with its own and refuses to open a newer store.

## Requests and responses

Each request gets exactly one response, in order.

Limits:

- The client gives up when connect plus handshake take over 2 s, when a quick request (`status`, `reload`, `issues`, `subscribe`, `shutdown`) takes over 10 s, or a work request (`scan_now`, `export_now`, `import_now`, `backup_now`, `reprice`, `refresh_quota`, `connect_keychain`) over 300 s. After a timeout the connection is unusable. Events of a subscription have no timeout.
- The daemon serves at most 16 connections; it closes further ones at once. It drops a client that does not read a response within 1 s.
- A response that would exceed 1 MiB is replaced by `error` / `failed`.
- A request still running at shutdown is answered `error` / `shutting_down`.

| Request | Response | Notes |
| --- | --- | --- |
| `status` | `status` | pid, versions, `started_at`, `last_scan_at`, `config_error`, enabled `tasks`, `subscribers`, one entry per sink. |
| `scan_now {agent?}` | `scanned` | Incremental scan at user-initiated priority. |
| `export_now {sink?}` | `exported` | Unknown sink: `error` / `bad_request`. A failing sink is reported in its summary, not as `error`. |
| `import_now {source?}` | `imported` | Reads the enabled `sqlite` sources now (or only `source`); per source `files_read`, `rows` written and the first `error`. Unknown or disabled source: `error` / `bad_request`. A failing source is reported in its summary, not as `error`. |
| `backup_now` | `backed_up` | Works when `backup.enabled` is off. |
| `reprice {all}` | `repriced {repriced}` | Re-estimates stored events with the active pricing snapshot: the unpriced ones, or every event with `all`. Reported costs never change. |
| `refresh_quota` | `quota_refreshed` | `error` / `disabled` when `quota.enabled` is off. Per profile `status`: `ok`, `signed_out`, `token_expired`, `keychain_denied` (the Keychain refused the credentials to the daemon), `forbidden` (HTTP 403), `rate_limited` (HTTP 429), `malformed`, `network_down`, `failed`, `backoff` (skipped after earlier failures; nothing stored). The store keeps only `ok`, `signed_out`, `token_expired`, `keychain_denied` and `failed`: every other failure is stored as `failed`. |
| `connect_keychain` | `quota_refreshed` | Reads each Claude profile's credentials again, showing the macOS consent prompt only for a Keychain item that refused the quiet read (the user can choose "Always Allow" for the daemon), drops retry delays, then polls as `refresh_quota`. Waits while the prompt is open. `error` / `disabled` when `quota.enabled` is off. |
| `reload` | `reloaded` | Invalid config: `error` / `invalid_config`, the old config stays active. |
| `issues` | `issues` | Open parse issues and the last `config_error`. Issue `kind`: `format_drift`, `unknown_version`, `protected_folder`, `missing_cwd`, `parse_error`, `integration_drift` (Superset's database has an unknown shape). |
| `shutdown` | `shutting_down` | Graceful stop, same as SIGTERM; the process exits 0, so launchd (`KeepAlive { SuccessfulExit = false }`) does not restart it. The answer comes first; the socket is removed, then `daemon.lock` released, when the daemon has stopped. `restore` and `erase` send it before they take `daemon.lock`. |
| `subscribe` | `subscribed`, then `event` lines | Events are interleaved with later responses on the same connection; a subscriber that falls behind gets one `store_changed` listing every dataset. |

Errors are `{"type":"error","kind":…,"message":…}` with `kind` one of `bad_request`, `disabled`, `invalid_config`, `failed`, `shutting_down`. Messages never contain credentials or tokens.

## Pushed events

Sent as `{"type":"event","event":{…}}` to subscribers only.

| Event | When |
| --- | --- |
| `store_changed {datasets}` | The head revision of a dataset moved. Datasets are collected and sent at most once per second; never while nothing changed. |
| `alert_fired {alert}` | An alert was written for the first time. Deduplicated alerts are not pushed again. |

Clients re-read the store after `store_changed`; the event carries no row data.

## Daemon behaviour

- File watching: one watcher over every agent root and the config file feeds a bounded channel of 1024 events. When the channel is full, further events are dropped and the next batch runs one full rescan instead, so no change is lost. An agent root that does not exist is checked every 60 s and watched once it appears; while every root exists and no file changes, the watcher never wakes.
- Shutdown (SIGTERM or `shutdown`): no new work starts, the socket is removed, running scans and reprices stop between two files or pages, sinks get a last export, then `daemon.lock` is released. The whole stop takes at most the shutdown grace (1.2 s); work still running then is abandoned and the last export skipped.

## Versioning

- `PROTOCOL_VERSION` is bumped for any change an older peer would misread: removing or renaming a message or field, changing a meaning, adding a required field.
- Adding an optional field is compatible. A new event, an unsolicited response type or a new value of a closed enum (a quota status, an issue kind) is not: older clients fail to parse it, so it bumps the version. v2 added `keychain_denied` and `integration_drift`; v3 added the `metrics_failing` alert kind; v4 added the `source_failing` alert kind and `import_now`. A new request with its own response is compatible: only a client that sends it can receive that response (an older daemon answers `error` / `bad_request`).
- There is no negotiation: a mismatch ends the connection. Ship the daemon and its clients together: they come from one app bundle, and the app shows "Update required" on `incompatible`.
