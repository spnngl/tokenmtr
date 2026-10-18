# docs (user-facing)

`README.md` and `docs/` are read by users and contributors who do not have `PLAN.md`: state each rule itself, with no PLAN section or D-number references.

## Parts checked by tests

Edit these from their source; the named test fails on drift and prints or names the fix.

| Part | Source | Test |
| --- | --- | --- |
| Table between the `schema:start`/`schema:end` markers in `export-schema.md` | `crates/tokenmtr-export/src/schema.rs` | `export_schema_doc_matches_the_code` (prints the replacement) |
| First `toml` block of `config.md` | `crates/tokenmtr-config/tests/fixtures/example.toml` (edit both) | `docs_embed_the_example_verbatim` |
| `status.schema.json` | `Status` and `Recap` in `crates/tokenmtr-query` | `apps/cli/tests/schema.rs`, `tests/e2e/tests/scenario.rs` |

## Adding a page

Add its row to the "Docs" table in `README.md`.
