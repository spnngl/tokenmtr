//! File formats: a codec turns text into a [`RawConfig`], resolves config paths to source
//! spans for error positions, and applies the structured edits that keep comments.

use std::ops::Range;

use camino::Utf8Path;
use tokenmtr_core::ProfileId;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, TableLike, Value};

use crate::error::{ConfigPath, Segment};
use crate::model::MenubarRing;
use crate::raw::{RawConfig, RawProject, RawSubscription};

/// The text did not decode into a [`RawConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// What the codec reports, for example "unknown field `foo`".
    pub message: String,
    /// Byte range in the source, when the codec knows it.
    pub span: Option<Range<usize>>,
}

/// An edit could not be applied to the document text.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The current text is not valid TOML.
    #[error("the file is not valid TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    /// The entry could not be written as TOML.
    #[error("cannot write the entry as TOML: {0}")]
    Serialize(#[from] toml_edit::ser::Error),
    /// The key exists but is not a `[[table array]]`.
    #[error("`{key}` exists but is not a [[{key}]] table array")]
    NotAnArrayOfTables {
        /// The top-level key.
        key: &'static str,
    },
    /// The key exists but is not a table.
    #[error("`{key}` exists but is not a [{key}] table")]
    NotATable {
        /// The top-level key.
        key: &'static str,
    },
}

/// A config file format. One implementation per format; [`ConfigFormat`] dispatches to them.
pub trait ConfigCodec {
    /// Parses `source` into the typed file contents.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for invalid syntax, unknown keys, wrong types and values rejected while
    /// parsing (durations, ids, enums).
    fn decode(&self, source: &str) -> Result<RawConfig, DecodeError>;

    /// The byte range of the value at `path`, or of its deepest existing ancestor.
    fn locate(&self, source: &str, path: &ConfigPath) -> Option<Range<usize>>;

    /// `source` with `entry` appended as a new `[[subscriptions]]` table; everything else, comments
    /// included, stays byte for byte.
    ///
    /// # Errors
    ///
    /// [`EditError`] when `source` cannot be edited.
    fn append_subscription(
        &self,
        source: &str,
        entry: &RawSubscription,
    ) -> Result<String, EditError>;

    /// `source` with `rule` appended as a new `[[projects]]` table (the lowest precedence, since
    /// the first match wins).
    ///
    /// # Errors
    ///
    /// [`EditError`] when `source` cannot be edited.
    fn append_project_rule(&self, source: &str, rule: &RawProject) -> Result<String, EditError>;

    /// `source` with `[menubar]` `rings` and `hidden_profiles` set; the table is created when
    /// missing, an existing value keeps its comment, and `hidden_profiles` is only written when
    /// present or non-empty.
    ///
    /// # Errors
    ///
    /// [`EditError`] when `source` cannot be edited.
    fn set_menubar_rings(
        &self,
        source: &str,
        rings: &[MenubarRing],
        hidden_profiles: &[ProfileId],
    ) -> Result<String, EditError>;
}

/// TOML, read with `toml` and edited with `toml_edit`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TomlCodec;

impl ConfigCodec for TomlCodec {
    fn decode(&self, source: &str) -> Result<RawConfig, DecodeError> {
        toml::from_str(source).map_err(|e| DecodeError {
            message: e.message().to_owned(),
            span: e.span(),
        })
    }

    fn locate(&self, source: &str, path: &ConfigPath) -> Option<Range<usize>> {
        let document = toml_edit::Document::parse(source).ok()?;
        let mut item = document.as_item();
        let mut span = item.span();
        for segment in path.segments() {
            let next = match segment {
                Segment::Key(key) => item.get(key.as_str()),
                Segment::Index(index) => item.get(*index),
            };
            let Some(next) = next else { break };
            item = next;
            span = item.span().or(span);
        }
        span
    }

    fn append_subscription(
        &self,
        source: &str,
        entry: &RawSubscription,
    ) -> Result<String, EditError> {
        append_table(source, "subscriptions", entry)
    }

    fn append_project_rule(&self, source: &str, rule: &RawProject) -> Result<String, EditError> {
        append_table(source, "projects", rule)
    }

    fn set_menubar_rings(
        &self,
        source: &str,
        rings: &[MenubarRing],
        hidden_profiles: &[ProfileId],
    ) -> Result<String, EditError> {
        let mut document: DocumentMut = source.parse()?;
        let menubar = document
            .as_table_mut()
            .entry("menubar")
            .or_insert_with(toml_edit::table)
            .as_table_like_mut()
            .ok_or(EditError::NotATable { key: "menubar" })?;
        set_array(menubar, "rings", rings.iter().map(|r| r.as_str()).collect());
        if !hidden_profiles.is_empty() || menubar.contains_key("hidden_profiles") {
            let profiles = hidden_profiles.iter().map(ProfileId::as_str).collect();
            set_array(menubar, "hidden_profiles", profiles);
        }
        Ok(document.to_string())
    }
}

/// Sets `key` to `array`, keeping the comment and spacing of an existing value.
fn set_array(table: &mut dyn TableLike, key: &str, array: Array) {
    let mut value = Value::Array(array);
    if let Some(old) = table.get(key).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    table.insert(key, Item::Value(value));
}

fn append_table<T: serde::Serialize>(
    source: &str,
    key: &'static str,
    entry: &T,
) -> Result<String, EditError> {
    let mut document: DocumentMut = source.parse()?;
    let array = document
        .as_table_mut()
        .entry(key)
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or(EditError::NotAnArrayOfTables { key })?;
    let mut table = toml_edit::ser::to_document(entry)?.into_table();
    table.decor_mut().set_prefix("\n");
    array.push(table);
    Ok(document.to_string())
}

/// The supported file formats. A new format adds one variant.
#[derive(Debug, Clone, Copy)]
pub enum ConfigFormat {
    /// TOML (`.toml`).
    Toml(TomlCodec),
}

impl ConfigFormat {
    /// The format named by `path`'s extension, or `None` when no format matches.
    #[must_use]
    pub fn from_path(path: &Utf8Path) -> Option<Self> {
        match path.extension()? {
            "toml" => Some(Self::Toml(TomlCodec)),
            _ => None,
        }
    }

    const fn codec(&self) -> &TomlCodec {
        match self {
            Self::Toml(codec) => codec,
        }
    }
}

impl ConfigCodec for ConfigFormat {
    fn decode(&self, source: &str) -> Result<RawConfig, DecodeError> {
        self.codec().decode(source)
    }

    fn locate(&self, source: &str, path: &ConfigPath) -> Option<Range<usize>> {
        self.codec().locate(source, path)
    }

    fn append_subscription(
        &self,
        source: &str,
        entry: &RawSubscription,
    ) -> Result<String, EditError> {
        self.codec().append_subscription(source, entry)
    }

    fn append_project_rule(&self, source: &str, rule: &RawProject) -> Result<String, EditError> {
        self.codec().append_project_rule(source, rule)
    }

    fn set_menubar_rings(
        &self,
        source: &str,
        rings: &[MenubarRing],
        hidden_profiles: &[ProfileId],
    ) -> Result<String, EditError> {
        self.codec()
            .set_menubar_rings(source, rings, hidden_profiles)
    }
}
