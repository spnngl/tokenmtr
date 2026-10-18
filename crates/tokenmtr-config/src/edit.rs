//! Editing a config file without losing its comments or layout.

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_core::SubscriptionSpec;

use crate::env::Env;
use crate::error::ConfigError;
use crate::format::{ConfigCodec, ConfigFormat};
use crate::load::parse;
use crate::model::Config;
use crate::raw::{RawProject, RawSubscription};

/// A config file's text plus the format that edits it.
///
/// Every edit changes only the text it adds: comments, key order and blank lines stay. An edit is
/// kept only when the result still validates, so [`ConfigDocument::text`] is always a loadable
/// config after a successful edit. Writing the text to disk is the caller's job; use
/// `tokenmtr_platform::fs::replace_atomically`.
#[derive(Debug, Clone)]
pub struct ConfigDocument {
    format: ConfigFormat,
    file: Option<Utf8PathBuf>,
    text: String,
}

impl ConfigDocument {
    /// A document over `text`; errors carry no file name.
    #[must_use]
    pub fn new(text: impl Into<String>, format: ConfigFormat) -> Self {
        Self {
            format,
            file: None,
            text: text.into(),
        }
    }

    /// Reads the file at `path`; the format follows its extension.
    ///
    /// # Errors
    ///
    /// [`ConfigError::UnsupportedFormat`] or [`ConfigError::Read`].
    pub fn open(path: &Utf8Path) -> Result<Self, ConfigError> {
        let format =
            ConfigFormat::from_path(path).ok_or_else(|| ConfigError::UnsupportedFormat {
                path: path.to_owned(),
                extension: path.extension().map(str::to_owned),
            })?;
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self {
            format,
            file: Some(path.to_owned()),
            text,
        })
    }

    /// The current text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The file this document was read from.
    #[must_use]
    pub fn file(&self) -> Option<&Utf8Path> {
        self.file.as_deref()
    }

    /// Validates the current text.
    ///
    /// # Errors
    ///
    /// As [`load`](crate::load).
    pub fn config(&self, env: &dyn Env) -> Result<Config, ConfigError> {
        parse(&self.text, self.format, self.file.as_deref(), env)
    }

    /// Appends a `[[subscriptions]]` entry for `spec`, with `fee` and `fee_currency` explicit.
    /// Returns the validated config of the new text.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Edit`] when the text cannot be edited; [`ConfigError::Invalid`] when the
    /// result breaks a rule (for example two subscriptions that overlap). The document is
    /// unchanged on error.
    pub fn add_subscription(
        &mut self,
        spec: &SubscriptionSpec,
        env: &dyn Env,
    ) -> Result<Config, ConfigError> {
        let text = self
            .format
            .append_subscription(&self.text, &RawSubscription::from(spec))?;
        self.commit(text, env)
    }

    /// Appends a `[[projects]]` rule after the existing ones: the first match wins, so a new rule
    /// has the lowest precedence. Returns the validated config of the new text.
    ///
    /// # Errors
    ///
    /// As [`ConfigDocument::add_subscription`]; for example an invalid glob.
    pub fn add_project_rule(
        &mut self,
        rule: &RawProject,
        env: &dyn Env,
    ) -> Result<Config, ConfigError> {
        let text = self.format.append_project_rule(&self.text, rule)?;
        self.commit(text, env)
    }

    fn commit(&mut self, text: String, env: &dyn Env) -> Result<Config, ConfigError> {
        let config = parse(&text, self.format, self.file.as_deref(), env)?;
        self.text = text;
        Ok(config)
    }
}
