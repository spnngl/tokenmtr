//! Key skeletons: a privacy-safe sample of a line the parser could not map (D42).

use serde::Serialize;
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 6;
const MAX_ARRAY_ITEMS: usize = 3;
const MAX_KEYS: usize = 64;
const MAX_KEY_CHARS: usize = 48;
const MAX_CHARS: usize = 4096;

/// A JSON line with every value replaced by its type name (`"string"`, `"number"`, `"bool"`,
/// `"null"`). Object keys are kept, so the shape is visible; no value ever is. Arrays keep at most
/// their first three distinct element shapes. Keys are sorted, so equal shapes print equally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct KeySkeleton(String);

impl KeySkeleton {
    /// Builds the skeleton of one line. Input that is not JSON gives the text `"<invalid json>"`.
    #[must_use]
    pub fn from_json(line: &[u8]) -> Self {
        let text = match serde_json::from_slice::<Value>(line) {
            Ok(value) => Value::to_string(&skeleton(&value, 0)),
            Err(_) => String::from("\"<invalid json>\""),
        };
        Self(truncate(text))
    }

    /// The skeleton as JSON text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn skeleton(value: &Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return Value::from("…");
    }
    match value {
        Value::Null => Value::from("null"),
        Value::Bool(_) => Value::from("bool"),
        Value::Number(_) => Value::from("number"),
        Value::String(_) => Value::from("string"),
        Value::Array(items) => {
            let mut shapes: Vec<Value> = Vec::new();
            for item in items.iter().take(MAX_ARRAY_ITEMS) {
                let shape = skeleton(item, depth + 1);
                if !shapes.contains(&shape) {
                    shapes.push(shape);
                }
            }
            Value::Array(shapes)
        }
        Value::Object(map) => {
            // Sorted explicitly: `serde_json/preserve_order` may be enabled by another crate in the
            // build, which would otherwise make the output follow input order.
            let mut entries: Vec<(String, Value)> = map
                .iter()
                .take(MAX_KEYS)
                .map(|(key, item)| {
                    let key: String = key.chars().take(MAX_KEY_CHARS).collect();
                    (key, skeleton(item, depth + 1))
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().collect::<Map<String, Value>>())
        }
    }
}

fn truncate(text: String) -> String {
    match text.char_indices().nth(MAX_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_values_by_types_and_keeps_keys() {
        let line = br#"{"b":[{"x":1},{"x":2},"s"],"a":"secret","n":null,"f":1.5,"t":true}"#;
        assert_eq!(
            KeySkeleton::from_json(line).as_str(),
            r#"{"a":"string","b":[{"x":"number"},"string"],"f":"number","n":"null","t":"bool"}"#
        );
    }

    #[test]
    fn invalid_json_has_no_content() {
        assert_eq!(
            KeySkeleton::from_json(b"{\"secret\": ").as_str(),
            "\"<invalid json>\""
        );
    }

    #[test]
    fn output_is_bounded() {
        let wide: String = (0..2000)
            .map(|i| format!("\"key{i}\":1"))
            .collect::<Vec<_>>()
            .join(",");
        let skeleton = KeySkeleton::from_json(format!("{{{wide}}}").as_bytes());
        assert!(skeleton.as_str().chars().count() <= MAX_CHARS + 1);
    }
}
