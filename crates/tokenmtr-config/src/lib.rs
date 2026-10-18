//! tokenmtr-config: the TOML configuration of `TokenMtr` (PLAN §6, `docs/config.md`).
//!
//! - [`load`] / [`load_str`] turn a file into a validated [`Config`], or a [`ConfigError`] that
//!   names the file, line and column.
//! - [`RawConfig`] is the file as typed data; [`Config::from_raw`] applies defaults and the
//!   cross-field rules.
//! - [`ConfigDocument`] adds subscriptions and project rules while keeping comments.
//! - [`ConfigFormat`] and [`ConfigCodec`] keep the file format swappable (D4).

mod build;
mod edit;
mod env;
mod error;
mod format;
mod load;
mod model;
#[expect(
    missing_docs,
    reason = "fields mirror the keys documented in docs/config.md"
)]
pub mod raw;

pub use edit::ConfigDocument;
pub use env::{Env, ProcessEnv};
pub use error::{ConfigError, ConfigPath, LineCol, Location, Rejection, Segment, Violation};
pub use format::{ConfigCodec, ConfigFormat, DecodeError, EditError, TomlCodec};
pub use load::{load, load_str, load_with};
pub use model::*;
pub use raw::{DurationError, RawConfig, RawProject, RawSubscription, parse_duration};
