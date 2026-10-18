//! Mapping between domain values and `SQLite` columns. Every read goes through here, so a corrupt
//! row surfaces as a typed error instead of a panic.

use std::str::FromStr;

use jiff::Timestamp;
use rusqlite::Row;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{CodecError, StoreError};

/// `u64` to the `INTEGER` it is stored as.
pub(crate) fn to_i64(value: u64, column: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::OutOfRange { column })
}

/// A `LIMIT` bound.
pub(crate) fn to_limit(limit: usize) -> Result<i64, StoreError> {
    i64::try_from(limit).map_err(|_| StoreError::OutOfRange { column: "limit" })
}

/// `INTEGER` back to the `u64` that was stored.
pub(crate) fn to_u64(value: i64, column: &'static str) -> Result<u64, CodecError> {
    u64::try_from(value).map_err(|_| CodecError::Value {
        column,
        value: value.to_string(),
    })
}

/// A timestamp as stored: UTC microseconds since the epoch.
pub(crate) fn to_us(at: Timestamp) -> i64 {
    at.as_microsecond()
}

/// The timestamp stored as UTC microseconds.
pub(crate) fn from_us(micros: i64) -> Result<Timestamp, CodecError> {
    Ok(Timestamp::from_microsecond(micros)?)
}

/// `dev` / `inode` are unsigned in the OS but `SQLite` only stores signed integers: the bits are
/// kept, so the value round-trips.
pub(crate) const fn u64_bits_to_i64(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

/// Inverse of [`u64_bits_to_i64`].
pub(crate) const fn i64_bits_to_u64(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

/// The name a serde unit enum variant is stored under.
pub(crate) fn enum_text<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value)? {
        Value::String(name) => Ok(name),
        other => Err(CodecError::Value {
            column: "enum",
            value: other.to_string(),
        }
        .into()),
    }
}

/// Parses a stored serde unit enum variant name.
pub(crate) fn parse_enum<T: DeserializeOwned>(
    name: String,
    column: &'static str,
) -> Result<T, CodecError> {
    serde_json::from_value(Value::String(name.clone())).map_err(|_| CodecError::Value {
        column,
        value: name,
    })
}

/// Typed access to columns by name.
pub(crate) trait RowExt {
    /// A string-backed identifier.
    fn id<T>(&self, column: &'static str) -> Result<T, StoreError>
    where
        T: FromStr,
        CodecError: From<T::Err>;

    /// A nullable string-backed identifier.
    fn id_opt<T>(&self, column: &'static str) -> Result<Option<T>, StoreError>
    where
        T: FromStr,
        CodecError: From<T::Err>;

    /// A timestamp.
    fn ts(&self, column: &'static str) -> Result<jiff::Timestamp, StoreError>;

    /// A nullable timestamp.
    fn ts_opt(&self, column: &'static str) -> Result<Option<jiff::Timestamp>, StoreError>;

    /// A non-negative integer.
    fn count(&self, column: &'static str) -> Result<u64, StoreError>;

    /// A serde unit enum stored by name.
    fn enumeration<T: DeserializeOwned>(&self, column: &'static str) -> Result<T, StoreError>;

    /// A nullable serde unit enum stored by name.
    fn enumeration_opt<T: DeserializeOwned>(
        &self,
        column: &'static str,
    ) -> Result<Option<T>, StoreError>;

    /// A boolean stored as 0 / 1.
    fn flag(&self, column: &'static str) -> Result<bool, StoreError>;

    /// A nullable boolean stored as 0 / 1.
    fn flag_opt(&self, column: &'static str) -> Result<Option<bool>, StoreError>;
}

impl RowExt for Row<'_> {
    fn id<T>(&self, column: &'static str) -> Result<T, StoreError>
    where
        T: FromStr,
        CodecError: From<T::Err>,
    {
        let text: String = self.get(column)?;
        text.parse::<T>().map_err(|e| CodecError::from(e).into())
    }

    fn id_opt<T>(&self, column: &'static str) -> Result<Option<T>, StoreError>
    where
        T: FromStr,
        CodecError: From<T::Err>,
    {
        let text: Option<String> = self.get(column)?;
        text.map(|t| t.parse::<T>().map_err(|e| CodecError::from(e).into()))
            .transpose()
    }

    fn ts(&self, column: &'static str) -> Result<Timestamp, StoreError> {
        Ok(from_us(self.get(column)?)?)
    }

    fn ts_opt(&self, column: &'static str) -> Result<Option<Timestamp>, StoreError> {
        let micros: Option<i64> = self.get(column)?;
        Ok(micros.map(from_us).transpose()?)
    }

    fn count(&self, column: &'static str) -> Result<u64, StoreError> {
        Ok(to_u64(self.get(column)?, column)?)
    }

    fn enumeration<T: DeserializeOwned>(&self, column: &'static str) -> Result<T, StoreError> {
        Ok(parse_enum(self.get(column)?, column)?)
    }

    fn enumeration_opt<T: DeserializeOwned>(
        &self,
        column: &'static str,
    ) -> Result<Option<T>, StoreError> {
        let name: Option<String> = self.get(column)?;
        Ok(name.map(|n| parse_enum(n, column)).transpose()?)
    }

    fn flag(&self, column: &'static str) -> Result<bool, StoreError> {
        Ok(self.get(column)?)
    }

    fn flag_opt(&self, column: &'static str) -> Result<Option<bool>, StoreError> {
        Ok(self.get(column)?)
    }
}
