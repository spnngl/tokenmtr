# Adding a source

A source reads the rows other TokenMtr machines exported and writes them into this machine's store.
`tokenmtr_import::import_pending` handles file discovery, cursors, paging, redaction, `import_run`
records and failure isolation; a source kind only opens a file and reads decoded rows. Work
through the steps in order and finish each one before the next.

Read first: [export-schema.md](export-schema.md), the rows a source kind reads back, and the crate
doc of `tokenmtr-import` for the model.

## The contract

An `ImportSource` is one opened file. It must:

1. **Read through a private copy.** A sync client rewrites the shared file at any moment. Fail a
   copy taken while the file moved with `ImportError::Busy`; it is recorded as a failed run and
   read at the next pass.
2. **Check before it reads.** Reject a schema version the build does not map
   (`ImportError::Version`) and a table that lacks a column of `tokenmtr_export::schema`
   (`ImportError::MissingColumn`). Extra columns are ignored.
3. **Hold one machine.** `machine()` names it; a row of another machine is
   `ImportError::SeveralMachines`.
4. **Stream in revision order.** `read` calls `on_page` every `page_size` rows and at the end;
   `false` from `on_page` stops the read. Cursors are revisions, so order is not optional.
5. **Decode by schema.** Use `tokenmtr_import::usage_event` and `tool_call` (or extend them); they
   skip `extra_usage` and put no row value in an error.
6. **Map permission errors.** Use `ImportError::io` for file operations so a macOS-refused folder
   is `PermissionDenied`.

## Steps

### 1. Config

Add a variant to `SourceTarget` in `crates/tokenmtr-config/src/model.rs`, its raw keys in
`crates/tokenmtr-config/src/raw.rs`, build it in `crates/tokenmtr-config/src/build/sources.rs`
(including the `same_files` clash rule), and document the keys in `docs/config.md`.

Done when `cargo test -p tokenmtr-config` passes, including the config docs test.

### 2. Implementation

Create `crates/tokenmtr-import/src/xxx.rs` with `XxxExport` implementing `ImportSource`.

Write its tests in `crates/tokenmtr-import/tests/`. Done when they pass:

- an export written by the matching sink comes back with equal values;
- a second pass changes nothing; a higher revision replaces, a lower one does not;
- a file that moved, an unsupported version or a missing column fails that file only.

### 3. Dispatch

Add the `ImportKind::Xxx(XxxExport)` variant and its arms in the `ImportSource for ImportKind` impl
(`crates/tokenmtr-import/src/source.rs`), then open it from `import_file` in
`crates/tokenmtr-import/src/run.rs` and map the config in `ImportSpec::from_config`.

Done when the compiler reports no non-exhaustive `match` in the workspace.

### 4. Fuzz

A new parser of outside bytes needs a fuzz target per `fuzz/AGENTS.md`; the row decoders already
have `export_row`.

### 5. Gates

Done when these pass:

```sh
cargo fmt -p tokenmtr-import --check
cargo clippy -p tokenmtr-import --all-targets -- -D warnings
cargo test -p tokenmtr-import
just fuzz-smoke
```
