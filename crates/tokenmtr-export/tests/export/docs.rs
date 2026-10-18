//! `docs/export-schema.md` embeds the table generated from the schema; it must not drift.

use tokenmtr_export::schema_markdown;

const DOCS: &str = include_str!("../../../../docs/export-schema.md");
const START: &str = "<!-- schema:start -->\n";
const END: &str = "<!-- schema:end -->";

#[test]
fn export_schema_doc_matches_the_code() {
    let embedded = DOCS
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(table, _)| table);
    let generated = schema_markdown();
    assert_eq!(
        embedded,
        Some(format!("\n{generated}\n").as_str()),
        "docs/export-schema.md drifted from `schema_markdown()`; replace the text between the \
         markers with:\n{generated}"
    );
}
