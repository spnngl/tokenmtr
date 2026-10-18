//! The `source_file` table: where ingestion stopped in each agent log file (D15).

use camino::Utf8PathBuf;
use jiff::Timestamp;
use rusqlite::{Connection, Row, named_params};
use tokenmtr_core::{AgentId, ProfileId, SourceFileId};

use crate::codec::{RowExt, i64_bits_to_u64, to_i64, to_us, u64_bits_to_i64};
use crate::error::StoreError;

/// Resume state of one source file.
///
/// A file is resumed only when `dev`, `inode`, `size >= offset` and the hash of the line ending
/// at `offset` still match; otherwise the ingestor reparses it from the start (D15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Absolute path of the file; the key.
    pub path: Utf8PathBuf,
    /// Agent the file belongs to.
    pub agent: AgentId,
    /// Profile the file belongs to.
    pub profile: ProfileId,
    /// Device the file lived on.
    pub dev: u64,
    /// Inode of the file.
    pub inode: u64,
    /// Size of the file when last read.
    pub size: u64,
    /// Modification time when last read.
    pub mtime: Timestamp,
    /// Offset of the first byte not yet ingested.
    pub offset: u64,
    /// SHA-256 of the last complete line before `offset`, when there is one.
    pub last_line_hash: Option<[u8; 32]>,
}

/// A checkpoint and the id events reference it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCheckpoint {
    /// The id stored in `usage_event.source_file_id`.
    pub id: SourceFileId,
    /// The resume state.
    pub checkpoint: Checkpoint,
}

const SAVE: &str = "INSERT INTO source_file \
    (path, agent, profile, dev, inode, size, mtime_us, byte_offset, last_line_hash, updated_at_us) \
    VALUES (:path, :agent, :profile, :dev, :inode, :size, :mtime_us, :byte_offset, \
    :last_line_hash, :updated_at_us) \
    ON CONFLICT (path) DO UPDATE SET agent = excluded.agent, profile = excluded.profile, \
    dev = excluded.dev, inode = excluded.inode, size = excluded.size, \
    mtime_us = excluded.mtime_us, byte_offset = excluded.byte_offset, \
    last_line_hash = excluded.last_line_hash, updated_at_us = excluded.updated_at_us \
    RETURNING id";

const LOAD: &str = "SELECT id, path, agent, profile, dev, inode, size, mtime_us, byte_offset, \
    last_line_hash FROM source_file WHERE agent = ?1 ORDER BY path";

/// Inserts or replaces the checkpoint of `checkpoint.path` and returns its stable id.
pub(crate) fn save(
    conn: &Connection,
    now: Timestamp,
    checkpoint: &Checkpoint,
) -> Result<SourceFileId, StoreError> {
    let id: i64 = conn.prepare_cached(SAVE)?.query_row(
        named_params! {
            ":path": checkpoint.path.as_str(),
            ":agent": checkpoint.agent.as_str(),
            ":profile": checkpoint.profile.as_str(),
            ":dev": u64_bits_to_i64(checkpoint.dev),
            ":inode": u64_bits_to_i64(checkpoint.inode),
            ":size": to_i64(checkpoint.size, "size")?,
            ":mtime_us": to_us(checkpoint.mtime),
            ":byte_offset": to_i64(checkpoint.offset, "offset")?,
            ":last_line_hash": checkpoint.last_line_hash,
            ":updated_at_us": to_us(now),
        },
        |row| row.get(0),
    )?;
    Ok(SourceFileId::new(id))
}

/// Every checkpoint of one agent, ordered by path.
pub(crate) fn load(conn: &Connection, agent: AgentId) -> Result<Vec<StoredCheckpoint>, StoreError> {
    let mut statement = conn.prepare_cached(LOAD)?;
    statement
        .query_and_then([agent.as_str()], decode)?
        .collect()
}

fn decode(row: &Row<'_>) -> Result<StoredCheckpoint, StoreError> {
    let path: String = row.get("path")?;
    Ok(StoredCheckpoint {
        id: SourceFileId::new(row.get("id")?),
        checkpoint: Checkpoint {
            path: path.into(),
            agent: row.id::<AgentId>("agent")?,
            profile: row.id::<ProfileId>("profile")?,
            dev: i64_bits_to_u64(row.get("dev")?),
            inode: i64_bits_to_u64(row.get("inode")?),
            size: row.count("size")?,
            mtime: row.ts("mtime_us")?,
            offset: row.count("byte_offset")?,
            last_line_hash: row.get("last_line_hash")?,
        },
    })
}
