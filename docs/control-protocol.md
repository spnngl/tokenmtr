# Control protocol v1

How clients (CLI, menu-bar app) talk to the daemon (D24, D52). Types live in `crates/tokenmtr-control/src/protocol.rs`; that file is the source of truth, this page is the map.

## Transport

- Unix domain socket at `<data_dir>/control.sock`, mode `0600`, parent directory forced to `0700`. The daemon accepts only peers with the socket owner's uid (`getpeereid`).
- Single instance: the daemon holds an exclusive `flock` on `<data_dir>/daemon.lock` for its whole life and takes it before it touches the socket or the store; a second daemon exits with `AlreadyRunning`. `restore`, `erase` and the CLI's direct scan/backup take the same lock, after stopping the service, so they never run beside a daemon.
- One JSON object per line (NDJSON): UTF-8, terminated by `\n`, no embedded newline, at most 1 MiB. A line that breaks framing (too long, not UTF-8) closes the connection; a line that is not a valid request after the handshake gets `error` / `bad_request` and the connection stays open.
- The `type` field names the variant, in snake case. Unknown fields are ignored by readers.

## Handshake

1. The client sends `{"type":"hello","protocol":1,"client":{"name":"…","version":"…","schema":null}}` first. Any other first message, or none within the handshake timeout, ends the connection (`error` / `bad_request` for a wrong first message).
2. Same `protocol`: the daemon answers `welcome` (`protocol`, `daemon_version`, `store_schema`).
3. Different `protocol`: the daemon answers `incompatible` (`daemon_protocol`, `daemon_version`, `store_schema`, `reason`), sends nothing else and closes. A client that reads the store directly compares `store_schema` with its own and refuses to open a newer store.

## Requests and responses

Each request gets exactly one response, in order.

Limits:

- The client gives up when connect plus handshake take over 2 s, when a quick request (`status`, `reload`, `issues`, `subscribe`, `shutdown`) takes over 10 s, or a work request (`scan_now`, `export_now`, `backup_now`, `refresh_quota`) over 300 s. After a timeout the connection is unusable. Events of a subscription have no timeout.
- The daemon serves at most 16 connections; it closes further ones at once. It drops a client that does not read a response within 1 s.
- A response that would exceed 1 MiB is replaced by `error` / `failed`.
- A request still running at shutdown is answered `error` / `shutting_down`.

| Request | Response | Notes |
| --- | --- | --- |
| `status` | `status` | pid, versions, `started_at`, `last_scan_at`, `config_error`, enabled `tasks`, `subscribers`, one entry per sink. |
| `scan_now {agent?}` | `scanned` | Incremental scan at user-initiated priority. |
| `export_now {sink?}` | `exported` | Unknown sink: `error` / `bad_request`. A failing sink is reported in its summary, not as `error`. |
| `backup_now` | `backed_up` | Works when `backup.enabled` is off. |
| `refresh_quota` | `quota_refreshed` | `error` / `disabled` when `quota.enabled` is off. Per profile `status`: `ok`, `signed_out`, `token_expired`, `malformed`, `network_down`, `failed`. |
| `reload` | `reloaded` | Invalid config: `error` / `invalid_config`, the old config stays active. |
| `issues` | `issues` | Open parse issues and the last `config_error`. |
| `shutdown` | `shutting_down` | Graceful stop, same as SIGTERM; the process exits 0, so launchd (`KeepAlive { SuccessfulExit = false }`) does not restart it. The answer comes first; the socket is removed, then `daemon.lock` released, when the daemon has stopped (D56). |
| `subscribe` | `subscribed`, then `event` lines | Events are interleaved with later responses on the same connection; a subscriber that falls behind gets one `store_changed` listing every dataset. |

Errors are `{"type":"error","kind":…,"message":…}` with `kind` one of `bad_request`, `disabled`, `invalid_config`, `failed`, `shutting_down`. Messages never contain credentials (D55).

## Pushed events

Sent as `{"type":"event","event":{…}}` to subscribers only.

| Event | When |
| --- | --- |
| `store_changed {datasets}` | The head revision of a dataset moved. Datasets are collected and sent at most once per second; never while nothing changed. |
| `alert_fired {alert}` | An alert was written for the first time (D36). Deduplicated alerts are not pushed again. |

Clients re-read the store after `store_changed`; the event carries no row data.

## Versioning

- `PROTOCOL_VERSION` is bumped for any change an older peer would misread: removing or renaming a message or field, changing a meaning, adding a required field.
- Adding an optional field is compatible. A new event or an unsolicited response type is not: v1 clients fail to parse it, so it bumps the version. A new request with its own response is compatible: only a client that sends it can receive that response (an older daemon answers `error` / `bad_request`).
- There is no negotiation: a mismatch ends the connection. Ship the daemon and its clients together (D52).
