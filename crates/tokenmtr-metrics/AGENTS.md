# tokenmtr-metrics

The crate doc in `src/lib.rs` states the model (what is counted, the labels, exclusion, sessions); `docs/metrics.md` is the contract scrapers read.

- This crate is pure: it reads a `StoreReader` and returns series. It opens no socket (network lives in `tokenmtr-remote`) and writes nothing.
- Metrics carry tokens, counts and quota utilization, never money. Never add `cwd` as a label.
- Adding a label: add it to `METRIC_LABELS` in `crates/tokenmtr-config/src/model.rs` (external labels may not collide with it) and to the family in `src/catalog.rs`; the tests that compare the two fail otherwise.
- Count rows only through `COUNTED` and the queries in `src/snapshot.rs`, so `superseded` rows, daily aggregates and extra-usage rows stay out.
- Group in Rust, not with `GROUP BY`: on 1,000,000 events SQLite sorts every row (2.3 s against 0.8 s for the whole snapshot).

Done when `cargo test -p tokenmtr-metrics` passes and `cargo bench -p tokenmtr-metrics --bench metrics` shows no regression.
