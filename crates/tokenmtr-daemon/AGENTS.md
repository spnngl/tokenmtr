# tokenmtr-daemon

The store's writer whenever it runs. The crate doc in `src/lib.rs` lists its tasks; the `Ctx` doc in `src/ctx.rs` gives the lock order; control requests are answered in `src/handler.rs`. File watching and shutdown behaviour are user-facing: `docs/control-protocol.md` "Daemon behaviour".

## Idle means asleep

- Timed work is a `Job` in the one scheduler (`src/scheduler.rs`); an idle daemon wakes only for a deadline, a control request or a file event. Exports are event-driven.
- Store work runs through `Ctx::blocking`: `ThreadPriority::Background` for scheduled work, `UserInitiated` for work a user waits on (control requests).
- Long work checks `Ctx::stopping` between files or pages, so shutdown stays within its grace.
- Network jobs run only while their config flag is on; `tests/network.rs` proves the daemon sends nothing when they are off.

## Adding a scheduled job

1. A variant in `Job` (`src/ctx.rs`).
2. Arm or cancel it in `src/reload.rs` when the config it depends on changes.
3. Run it in `run_job` (`src/daemon.rs`) and re-arm it when it finishes (`schedule_periodic` for a fixed period).
4. Test it in `src/daemon/tests.rs` or `tests/daemon.rs`.

Done when `cargo test -p tokenmtr-daemon` passes.
