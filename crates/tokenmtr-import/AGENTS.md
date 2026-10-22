# tokenmtr-import

The crate doc in `src/lib.rs` is the model: files, private copy, cursor, pages, failure rule.

- Adding a source kind: `docs/adding-a-source.md`.
- Imported rows land in `usage_event` and `tool_call` with `origin_machine` set (see `crates/tokenmtr-store/AGENTS.md`); this crate writes no local rows.
- The reader opens a private copy of the file read-only, because a sync client rewrites the original at any moment.
- A failure fails one file and becomes an `import_run` row. Keep row values out of error messages.
- Every column is read by name in `tokenmtr_export::schema` order. Changing an export column changes `src/decode.rs` in the same change; `SUPPORTED_VERSIONS` in `src/sqlite.rs` gains the new `EXPORT_SCHEMA_VERSION` once the decoder maps it.
- The decoders parse outside data: the `export_row` target in `fuzz/` drives them. Extend its seeds (`scripts/fuzz-seeds.nu`) when a column is added.

Done when `cargo test -p tokenmtr-import` and `just fuzz-smoke` pass.
