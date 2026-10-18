# tokenmtr-core

Domain types and pure rules. The module docs of `src/money.rs`, `src/pricing.rs` and `src/ids.rs` hold the details; these are the invariants other crates and stored data depend on.

## Invariants

- Money is integer micro-units (`UsdMicros`, `EurMicros`). A float enters once, through `from_f64` at a parse boundary; rates and prices go through `rust_decimal`. Sums stay exact, so totals match across screens, reports and exports.
- USD is the only stored currency. EUR is derived at read time from FX rates, so a new rate never rewrites history.
- `EventId::new` is permanent: `event_id` is the exported primary key, documented to consumers as stable per machine, agent and dedup key. A sink that outlives an erase or a reinstall merges re-ingested events only while the derivation holds.
- An event no catalog entry or override prices stays unpriced; it is never guessed.

## Changing estimation

Stored events keep their estimate and `pricing_version` until their inputs change (`crates/tokenmtr-agents/src/ingest/pricing.rs`). A change to estimation logic or rates therefore reaches history only through `tokenmtr reprice --all`.

## Updating the vendored price list

1. Replace `pricing/litellm.json` with LiteLLM's `model_prices_and_context_window.json` at a new commit.
2. Set `commit`, `fetched` (today) and `source` in `pricing/litellm.lock`. The `fetched` date decides whether an installed app swaps its stored snapshot for this one.
3. For a Claude model LiteLLM dropped, add its entry to `pricing/legacy.json`.

Done when `cargo test -p tokenmtr-core` passes (`vendored_snapshot_parses_and_prices_opus` checks the file).
