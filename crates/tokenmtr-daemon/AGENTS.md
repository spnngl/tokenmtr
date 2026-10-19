# tokenmtr-daemon

The store's writer whenever it runs. The crate doc in `src/lib.rs` lists its tasks; the `Ctx` doc in `src/ctx.rs` gives the lock order; control requests are answered in `src/handler.rs`. File watching and shutdown behaviour are user-facing: `docs/control-protocol.md` "Daemon behaviour".

## Idle means asleep

- Timed work is a `Job` in the one scheduler (`src/scheduler.rs`); an idle daemon wakes only for a deadline, a control request or a file event. Exports are event-driven.
- Store work runs through `Ctx::blocking`: `ThreadPriority::Background` for scheduled work, `UserInitiated` for work a user waits on (control requests).
- Long work checks `Ctx::stopping` between files or pages, so shutdown stays within its grace.
- Network jobs run only while their config flag is on; `tests/network.rs` proves the daemon sends nothing when they are off.
- Metrics outputs exist only while a `[[metrics.*]]` entry does: with none, nothing is bound, computed or recorded (`tests/metrics.rs`). `Ctx::arm_metrics` is state-driven (it keeps the server, `Seen` set and timers of an unchanged output), the refresh is rate-limited to `refresh_interval` and woken by changed datasets, and a session's expiry arms its own refresh.
- A failing output retries with `Timing.metrics_backoff` doubling up to `export_retry_cap`, shortened by `before_alert` so the `metrics_failing` alert fires on time. Secrets are resolved at bind or push time, never at config load, and never logged.

## Adding a scheduled job

1. A variant in `Job` (`src/ctx.rs`).
2. Arm or cancel it in `src/reload.rs` when the config it depends on changes.
3. Run it in `run_job` (`src/daemon.rs`) and re-arm it when it finishes (`schedule_periodic` for a fixed period).
4. Test it in `src/daemon/tests.rs` or `tests/daemon.rs`.

Done when `cargo test -p tokenmtr-daemon` passes.
