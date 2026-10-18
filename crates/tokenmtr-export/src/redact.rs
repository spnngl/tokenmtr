//! Per-sink redaction, applied between the batch query and the sink.
//!
//! `hash` replaces a value by the first 16 hex characters of its blake3 hash keyed with the
//! install's redaction key: equal inputs give equal hashes (grouping works), and without the key
//! nobody can test hashes against a list of known paths. `drop` empties the cell; the column stays
//! so every sink of a dataset has the same shape.

use std::collections::BTreeMap;

use tokenmtr_config::{RedactColumn, RedactMode};
use tokenmtr_store::RedactionKey;

use crate::schema::{ExportRow, Value, schema};
use crate::sink::ExportBatch;

/// The keyed hash of `value`: 16 lowercase hex characters ([`RedactionKey::hash`]).
#[must_use]
pub fn hash_value(key: &RedactionKey, value: &str) -> String {
    key.hash(value)
}

/// A sink's redaction rules bound to the install key.
#[derive(Debug, Clone)]
pub struct Redactor<'a> {
    key: &'a RedactionKey,
    rules: &'a BTreeMap<RedactColumn, RedactMode>,
}

impl<'a> Redactor<'a> {
    /// Binds `rules` to `key`.
    #[must_use]
    pub const fn new(key: &'a RedactionKey, rules: &'a BTreeMap<RedactColumn, RedactMode>) -> Self {
        Self { key, rules }
    }

    /// Redacts every row of the batch in place. Empty cells stay empty: there is nothing to hide.
    pub fn apply(&self, batch: &mut ExportBatch) {
        if self.rules.is_empty() {
            return;
        }
        let columns = schema(batch.dataset).columns;
        let actions: Vec<Option<RedactMode>> = columns
            .iter()
            .map(|c| c.redact.and_then(|r| self.rules.get(&r).copied()))
            .collect();
        for row in &mut batch.rows {
            self.apply_row(&actions, row);
        }
    }

    fn apply_row(&self, actions: &[Option<RedactMode>], row: &mut ExportRow) {
        for (value, action) in row.values.iter_mut().zip(actions) {
            let Some(mode) = action else { continue };
            match (mode, &*value) {
                (_, Value::Null) => {}
                (RedactMode::Hash, Value::Text(text)) => {
                    *value = Value::Text(hash_value(self.key, text));
                }
                // Redactable columns are text; any other cell is emptied rather than leaked.
                (RedactMode::Drop | RedactMode::Hash, _) => *value = Value::Null,
            }
        }
    }
}
