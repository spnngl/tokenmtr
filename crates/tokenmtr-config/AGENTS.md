# tokenmtr-config

`docs/config.md` is the user contract: `src/raw.rs` mirrors its tables one to one and `src/build.rs` validates in its order.

## Adding or changing a key

1. Shape: the field in its `src/raw.rs` struct (unknown keys stay rejected), the typed value in `src/model.rs`, the default and cross-field rules in `src/build.rs`.
2. Docs: the key in its table under "Keys" in `docs/config.md`, and in the example, which must stay byte-identical to `tests/fixtures/example.toml`. Done when `docs_embed_the_example_verbatim` passes.
3. Fuzz seeds: run `just fuzz-seeds`; the `config` seed is that example.
4. Live reload: when the daemon acts on the key, react to its change in `crates/tokenmtr-daemon/src/reload.rs`.
5. App edits: when Settings writes the key, add a `ConfigDocument` method in `src/edit.rs` over a `ConfigCodec` edit in `src/format.rs`, which keeps the file's comments and layout.

Done when `cargo test -p tokenmtr-config` and `just fuzz-smoke` pass.
