//! The `meta` table: identity seeded by the first migration, plus the daemon heartbeat.

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension};
use tokenmtr_core::MachineId;

use crate::codec::{from_us, to_us};
use crate::error::{CodecError, StoreError};
use crate::types::RedactionKey;

/// Who this database belongs to.
#[derive(Debug, Clone)]
pub struct Identity {
    pub(crate) machine_id: MachineId,
    pub(crate) redaction_key: RedactionKey,
}

/// Reads the rows the first migration seeds.
pub(crate) fn load_identity(conn: &Connection) -> Result<Identity, StoreError> {
    let machine: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'machine_id'", [], |r| {
            r.get(0)
        })
        .optional()?;
    let machine = machine.ok_or(StoreError::MissingMeta("machine_id"))?;
    let key: Option<[u8; 32]> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'redaction_key'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Identity {
        machine_id: machine.parse().map_err(CodecError::from)?,
        redaction_key: RedactionKey::new(key.ok_or(StoreError::MissingMeta("redaction_key"))?),
    })
}

/// When the daemon last said it was alive.
pub(crate) fn heartbeat(conn: &Connection) -> Result<Option<Timestamp>, StoreError> {
    let micros: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = 'heartbeat'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(micros.map(from_us).transpose()?)
}

/// Records that the daemon is alive at `now`.
pub(crate) fn set_heartbeat(conn: &Connection, now: Timestamp) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('heartbeat', ?1) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [to_us(now)],
    )?;
    Ok(())
}
