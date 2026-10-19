# fuzz

One target per parser of outside data; each must accept any bytes without panicking. `fuzz/` is its own Cargo workspace, so `just check` skips it: its gate is `just fuzz-smoke` (stable), and `just fuzz <target>` runs a real session (nightly, `cargo-fuzz`).

## Adding a target

1. The entry function in `src/lib.rs` and its row in `TARGETS`.
2. A one-line shim `fuzz_targets/<name>.rs` like the others, and its `[[bin]]` in `Cargo.toml`.
3. A `write-seeds` call for it in `scripts/fuzz-seeds.nu`, then `just fuzz-seeds`.

Done when `just fuzz-smoke` passes: it checks that `TARGETS`, `fuzz_targets/` and `seeds/` name the same targets.

## After a fixture changes

Run `just fuzz-seeds` and commit `seeds/`: the seeds are copies of `fixtures/` and the crates' test fixtures. Transcripts are globbed, but HTTP and config fixtures are listed by name in `scripts/fuzz-seeds.nu`: add a new one there first.

## Fixing a crash

`just fuzz-smoke` writes the failing input under `target/fuzz-smoke-crash/`. Fix the parser, add the input as a fixture with a regression test in the owning crate, then refresh the seeds.
