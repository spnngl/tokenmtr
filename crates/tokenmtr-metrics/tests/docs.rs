//! `docs/metrics.md` embeds the table generated from the catalog; it must not drift.

use tokenmtr_metrics::catalog::catalog_markdown;

const DOCS: &str = include_str!("../../../docs/metrics.md");
const START: &str = "<!-- metrics:start -->\n";
const END: &str = "<!-- metrics:end -->";

#[test]
fn metrics_doc_matches_the_catalog() {
    let embedded = DOCS
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(table, _)| table);
    let generated = catalog_markdown();
    assert_eq!(
        embedded,
        Some(format!("\n{generated}\n").as_str()),
        "docs/metrics.md drifted from `catalog_markdown()`; replace the text between the \
         markers with:\n{generated}"
    );
}
