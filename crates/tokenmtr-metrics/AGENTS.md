# tokenmtr-metrics

The crate doc in `src/lib.rs` states the model (what is counted, the labels, exclusion, sessions); `docs/metrics.md` is the contract scrapers read.

- This crate is pure: it reads a `StoreReader` and returns series. It opens no socket (network lives in `tokenmtr-remote`) and writes nothing.
- Metrics carry tokens, counts and quota utilization, never money. Never add `cwd` as a label.
- Adding a label: add it to `METRIC_LABELS` in `crates/tokenmtr-config/src/model.rs` (external labels may not collide with it) and to the family in `src/catalog.rs`; the tests that compare the two fail otherwise.
- Adding a metric: a `Family` variant (declared in name order) with its `name()`, `kind()`, `help()`, `labels()` and `source()` in `src/catalog.rs`; compute it in `src/snapshot.rs` and emit it in `Snapshot::fold` (`src/scope.rs`); then run the tests, which check the name rules, the sorted labels and `docs/metrics.md` (`metrics_doc_matches_the_catalog`; regenerate the table with `catalog_markdown()`). A per-session metric is a `Session*` twin of a total family: a usage or tool family reruns its query with `sessions = true`; a family read whole from a small table (signals) marks each row `active` and keeps only those.
- Cardinality: a total family never carries `session_id`, `git_branch` or any label with an unbounded value set; those belong to the `Session*` families, which exist only for sessions inside `[metrics].session_window`.
- Counters are absolute totals from the store (never deltas) and carry `created` (the earliest event) so OpenMetrics can send `_created`.
- Count rows only through `COUNTED` and the queries in `src/snapshot.rs`, so `superseded` rows, daily aggregates and extra-usage rows stay out.
- Group `usage_event` and `tool_call` rows in Rust, not with `GROUP BY`: on 1,000,000 events SQLite sorts every row (2.3 s against 0.8 s for the whole snapshot). `GROUP BY` is fine on a small table or one walked in grouping order by an index (`session_signal` by its unique key); the plan test in `src/tests.rs` checks the index.

Done when `cargo test -p tokenmtr-metrics` passes and `cargo bench -p tokenmtr-metrics --bench metrics` shows no regression.
