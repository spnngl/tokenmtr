# tokenmtr-export

The crate doc in `src/lib.rs` maps the pipeline; `docs/export-schema.md` is the contract consumers read.

- Adding a sink kind: follow `docs/adding-a-sink.md` (the sink contract, then the steps).
- Export rows carry stored values only: USD amounts, flags, revisions. EUR and billing mode are derived at query time, so consumers join `fx_rates` themselves.

## Changing a column

`src/schema.rs` names every column once; CSV, JSONL, `SQLite` DDL and the docs table derive from it.

1. Edit the dataset's column list in `src/schema.rs` and its mapping in `src/project.rs`.
2. Raise `EXPORT_SCHEMA_VERSION` for any column added, removed, renamed or retyped: an `SQLite` sink refuses a file of another version.
3. Replace the table between the `schema:start` and `schema:end` markers in `docs/export-schema.md` with the text `export_schema_doc_matches_the_code` prints.
4. Review the changed snapshots in `tests/export/snapshots/`.

Done when `cargo test -p tokenmtr-export` passes.
