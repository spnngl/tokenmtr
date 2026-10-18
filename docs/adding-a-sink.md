# Adding a sink

A sink receives batches of one dataset's rows and stores them durably. The pump
(`tokenmtr_export::export_pending`) handles cursors, redaction, batching, `sink_run` records and
failure isolation; a sink only implements `write`. Work through the steps in order and finish each
one before the next.

Read first: [export-schema.md](export-schema.md) for the rows a sink receives.

## The contract

`Sink::write(&mut self, batch: &ExportBatch) -> Result<(), SinkError>` must:

1. **Be durable on `Ok`.** The pump advances the cursor right after `Ok`; a crash after that loses
   nothing only if the rows are on stable storage (fsync a file, commit a transaction with
   `synchronous=FULL`).
2. **Be idempotent.** The same batch can arrive again after a crash between delivery and cursor
   advance. Delivering a row at its stored or a lower `revision` changes nothing; a higher
   `revision` replaces or supersedes the stored row.
3. **Use the schema, not literals.** Take column names, order, types and the primary key from
   `tokenmtr_export::schema(dataset)`; never write a column list by hand.
4. **Create resources lazily.** `new` touches no disk or network, so a refused path or an
   unreachable server is a failed run (visible in health), not a failed start.
5. **Map permission errors.** A refused folder returns `SinkError::PermissionDenied` (D50), built
   with `SinkError::io` for file operations.

Batches arrive in ascending `revision` order, already redacted, and are never empty.

## Steps

### 1. Config

Add a variant to `SinkTarget` in `crates/tokenmtr-config/src/model.rs` with its settings, build it
in `crates/tokenmtr-config/src/build.rs`, and document the keys in `docs/config.md`.

Done when `cargo test -p tokenmtr-config` passes, including the config docs test.

### 2. Target

In `crates/tokenmtr-export/src/sink.rs` add a resolved `XxxTarget` struct (defaults filled in), a
`Target::Xxx` variant, and the arm in `SinkSpec::from_config`.

Done when `cargo test -p tokenmtr-export spec` passes with a test for your defaults.

### 3. Implementation

Create `crates/tokenmtr-export/src/xxx.rs` with `XxxSink` implementing `Sink`. Generate any DDL or
wire format from `DatasetSchema`; share generators in `sql.rs` when the target speaks SQL.

Done when the sink passes its own tests in `crates/tokenmtr-export/tests/export/xxx.rs`:

- the same batch delivered twice leaves one row per primary key;
- a lower revision never overwrites a higher one, a higher one does;
- a refused destination returns `PermissionDenied` or `Io`, never panics;
- the stored columns equal `schema(dataset).columns` in name, order and type.

### 4. Dispatch

Add the `SinkKind::Xxx(XxxSink)` variant, its arm in `SinkKind::new`, and its arms in the
`Sink for SinkKind` impl. Re-export the new types from `crates/tokenmtr-export/src/lib.rs`.

Done when the compiler reports no non-exhaustive `match` in the workspace.

### 5. Pump coverage

Add your sink to the crash test `crash_between_delivery_and_ack_redelivers_idempotently` in
`crates/tokenmtr-export/tests/export/pump.rs`: the sink's current state after a crash and a
re-export equals the other sinks'.

Done when that test passes with your sink included.

### 6. Gates

Done when these pass:

```sh
cargo fmt -p tokenmtr-export --check
cargo clippy -p tokenmtr-export --all-targets -- -D warnings
cargo test -p tokenmtr-export
```

If you changed a column, bump `EXPORT_SCHEMA_VERSION` and regenerate the table in
`export-schema.md` from `schema_markdown()`; the docs test prints the text to paste.
