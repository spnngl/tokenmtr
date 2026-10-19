# tokenmtr-store

The crate doc in `src/lib.rs` is the model: connections, schema version, units, revisions and module layout. Read it before changing SQL. Each column of the schema is commented in `migrations/`.

## Guardrails

- One writer at a time: the daemon, or a `tokenmtr-app` action (direct scan, backup, reprice, erase, restore) that holds the daemon lock while no daemon runs. Everything else reads through `StoreReader`.
- Shipped migrations are immutable: installed stores already ran them, and `PRAGMA user_version` counts them. Change the schema by appending a migration.
- Exported datasets lose no row. Retract by updating a flag column (`superseded`, `removed_at_us`) so sinks receive the change as a new revision; sinks have no delete channel.
- A value column of an exported dataset is listed in its module's `VALUES` table (for example `src/events.rs`) with its `Merge` rule. The upsert compares those columns to decide whether a row earns a new revision: a column left out never reaches sinks when it changes.
- Store instants as UTC `*_us`, money as `*_usd_micros`. Zone buckets and EUR are computed by `tokenmtr-query`, so a zone or rate change never rewrites rows.

## Query plans

- Planner statistics come only from `Store::optimize()` (writer open, drop, and the daemon scan job); the doc of `optimize` in `src/db.rs` says why it never samples and empties `sqlite_stat4`. Readers are `query_only` and cannot refresh them.
- Filtering on `agent = ?` alone makes the planner pick the `(agent, dedup_key)` unique index and read the agent's whole history. Seek `(agent, profile, …)` indexes one distinct value at a time instead (see `LAST_ACTIVITY_OF_AGENT` in `tokenmtr-query`).
- Keep plan-sensitive SQL in `pub(crate)` constants that a test checks with `EXPLAIN QUERY PLAN`.
- Before changing a pragma, an index or a hot query, measure with the divan bench in `crates/tokenmtr-query/benches/queries.rs` (`just bench`; README explains `TOKENMTR_BENCH_STORE`).

## Adding a migration

1. Add `migrations/000N_<what>.sql` with the next number, commenting each new column or index the way earlier files do.
2. Append it to `migrations()` in `src/migrations.rs` and raise `SCHEMA_VERSION` by one.
3. A new value column of an exported dataset: add it to the module's `VALUES` and to its writer and reader, then follow "Changing a column" in `crates/tokenmtr-export/AGENTS.md`.
4. A new value of a closed text column (a `CHECK (... IN (...))` list such as `quota_snapshot.status` or `project_map.source`): SQLite cannot alter a `CHECK`, so rebuild the table the way `0006_inferred_project_keychain_denied.sql` does. A value that crosses the control channel also bumps the protocol (`docs/control-protocol.md` "Versioning").

Done when `cargo test -p tokenmtr-store` passes (`schema_version_counts_the_migrations`, `applying_twice_changes_nothing`). A build with the new version makes older apps and CLIs refuse the file (`StoreError::NewerSchema`), so the app bundle ships the daemon and its clients together.
