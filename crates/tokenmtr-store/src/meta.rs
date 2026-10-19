//! The `meta` table: identity seeded by the first migration, the daemon heartbeat and the time
//! each agent's last scan finished.

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use tokenmtr_core::{AgentId, MachineId};

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

/// Key prefix of the per-agent scan times; the agent id follows.
const SCAN_FINISHED: &str = "last_scan_finished_at.";

/// When the newest scan of any agent finished.
pub(crate) fn last_scan_finished_at(conn: &Connection) -> Result<Option<Timestamp>, StoreError> {
    let micros: Option<i64> = conn.query_row(
        "SELECT MAX(value) FROM meta WHERE key LIKE ?1",
        [format!("{SCAN_FINISHED}%")],
        |r| r.get(0),
    )?;
    Ok(micros.map(from_us).transpose()?)
}

/// Records that a scan of `agent` finished at `at`, whether or not it read anything.
pub(crate) fn set_scan_finished(
    conn: &Connection,
    agent: AgentId,
    at: Timestamp,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![format!("{SCAN_FINISHED}{}", agent.as_str()), to_us(at)],
    )?;
    Ok(())
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
