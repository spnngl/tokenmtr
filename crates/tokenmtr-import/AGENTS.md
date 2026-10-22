# tokenmtr-import

The crate doc in `src/lib.rs` is the model: files, private copy, cursor, pages, failure rule.

- Adding a source kind: follow `docs/adding-a-source.md` (the contract, then the steps).
- A source holds other machines' rows. Imported rows land in `usage_event` and `tool_call` with `origin_machine` set (see `crates/tokenmtr-store/AGENTS.md`); nothing here writes local rows.
- The reader never opens the shared file: it copies it, checks the copy, and opens the copy read-only. Keep it so; a sync client rewrites the original at any moment.
- A failure fails one file and becomes an `import_run` row. Never raise a parse issue, never abort the pass, never put a row value in an error message.
- Every column is read by name and in `tokenmtr_export::schema` order. Changing an export column means changing `src/decode.rs` in the same change, and `SUPPORTED_VERSIONS` in `src/sqlite.rs` gains the new `EXPORT_SCHEMA_VERSION` only when the decoder maps it.
- The decoders parse outside data: `fuzz/` target `export_row` drives them. Extend its seeds (`scripts/fuzz-seeds.nu`) when a column is added.

Done when `cargo test -p tokenmtr-import` and `just fuzz-smoke` pass.
