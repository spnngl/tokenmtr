# tokenmtr-query

The crate doc in `src/lib.rs` is the reference: inputs, time bucketing, money rules and the JSON shape of every result. Keep it current when a rule or a shape changes.

## Changing a result's JSON shape

The serde output of these types is a contract with the UI, and `Status` and `Recap` also with scripts.

1. Change the type, then set the new field in the matching value of `apps/desktop/src-tauri/src/goldens.rs` and run `just goldens`. Done when the fixture diff under `apps/desktop/src/shared/schemas/__fixtures__/` shows exactly your change.
2. Mirror it in the Zod schema under `apps/desktop/src/shared/schemas/`. Done when `just check-ui` passes.
3. For `Status` or `Recap`, follow "Stability" in `docs/cli-json.md`: within a `STATUS_SCHEMA` version fields are only added. Add the field to `docs/status.schema.json`. Done when `cargo test -p tokenmtr --test schema` passes.
