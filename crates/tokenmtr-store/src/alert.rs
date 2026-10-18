//! The `alert` table: fired alerts, unique by dedup key (D36).

use std::collections::HashSet;

use rusqlite::{Connection, named_params};
use serde_json::Value;
use tokenmtr_core::Alert;

use crate::codec::to_us;
use crate::error::{CodecError, StoreError};

// Only a repeated dedup key is skipped; any other constraint failure is an error.
const INSERT: &str = "INSERT INTO alert (kind, dedup_key, fired_at_us, payload) \
    VALUES (:kind, :dedup_key, :fired_at_us, :payload) ON CONFLICT (dedup_key) DO NOTHING";

/// Stores a fired alert; `false` when its dedup key was already stored.
pub(crate) fn save(conn: &Connection, alert: &Alert) -> Result<bool, StoreError> {
    let payload = serde_json::to_value(&alert.kind)?;
    let kind = payload
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CodecError::Value {
            column: "kind",
            value: payload.to_string(),
        })?;
    let inserted = conn.prepare_cached(INSERT)?.execute(named_params! {
        ":kind": kind,
        ":dedup_key": alert.dedup_key,
        ":fired_at_us": to_us(alert.at),
        ":payload": payload.to_string(),
    })?;
    Ok(inserted == 1)
}

/// Dedup keys of every alert fired so far. Not scoped by time: a sink can fail, or a drift stay
/// open, for longer than any horizon, and its key must still suppress a repeat. The table grows
/// by a few rows a day.
pub(crate) fn fired_keys(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut statement = conn.prepare_cached("SELECT dedup_key FROM alert")?;
    let keys = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(keys)
}
