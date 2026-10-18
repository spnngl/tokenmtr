//! Reading a config from a file or from memory.

use camino::Utf8Path;

use crate::env::{Env, ProcessEnv};
use crate::error::{ConfigError, LineCol, Location};
use crate::format::{ConfigCodec, ConfigFormat};
use crate::model::Config;

/// Reads and validates the config at `path`; the format follows the file extension. `~` expands
/// through the process environment.
///
/// # Errors
///
/// [`ConfigError`] naming the file and, when the codec can tell, the line and column.
pub fn load(path: &Utf8Path) -> Result<Config, ConfigError> {
    load_with(path, &ProcessEnv)
}

/// [`load`] with an explicit environment.
///
/// # Errors
///
/// As [`load`].
pub fn load_with(path: &Utf8Path, env: &dyn Env) -> Result<Config, ConfigError> {
    let format = ConfigFormat::from_path(path).ok_or_else(|| ConfigError::UnsupportedFormat {
        path: path.to_owned(),
        extension: path.extension().map(str::to_owned),
    })?;
    let source = fs_err(path, std::fs::read_to_string(path))?;
    parse(&source, format, Some(path), env)
}

/// Validates config text held in memory; errors carry no file name.
///
/// # Errors
///
/// [`ConfigError::Syntax`] or [`ConfigError::Invalid`] with a line and column.
pub fn load_str(source: &str, format: ConfigFormat, env: &dyn Env) -> Result<Config, ConfigError> {
    parse(source, format, None, env)
}

fn fs_err<T>(path: &Utf8Path, result: std::io::Result<T>) -> Result<T, ConfigError> {
    result.map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })
}

/// Decodes and validates `source`, attaching `file` and positions to every error.
pub(crate) fn parse(
    source: &str,
    format: ConfigFormat,
    file: Option<&Utf8Path>,
    env: &dyn Env,
) -> Result<Config, ConfigError> {
    let location = |offset: Option<usize>| Location {
        file: file.map(Utf8Path::to_owned),
        position: offset.map(|offset| LineCol::at(source, offset)),
    };
    let raw = format.decode(source).map_err(|e| ConfigError::Syntax {
        at: location(e.span.map(|span| span.start)),
        message: e.message,
    })?;
    Config::from_raw(&raw, env).map_err(|rejection| ConfigError::Invalid {
        at: location(
            format
                .locate(source, &rejection.path)
                .map(|span| span.start),
        ),
        path: rejection.path,
        violation: Box::new(rejection.violation),
    })
}
