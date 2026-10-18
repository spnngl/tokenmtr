//! The `pricing_snapshot` table: every pricing body ever used, so estimates stay reproducible
//! (D11). The active snapshot is `meta.pricing_version`.

use jiff::Timestamp;
use rusqlite::{Connection, TransactionBehavior, named_params};
use tokenmtr_core::PricingVersion;

use crate::codec::{RowExt, to_i64, to_u64, to_us};
use crate::error::{CodecError, StoreError};

/// zstd level for snapshot bodies: they are written rarely and read rarely.
const ZSTD_LEVEL: i32 = 3;

/// A stored pricing snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPricingSnapshot {
    /// The first 12 hex characters of the body's SHA-256.
    pub version: PricingVersion,
    /// When the body was fetched.
    pub fetched_at: Timestamp,
    /// Where the body came from (a URL, or `embedded`).
    pub source: Option<String>,
    /// The decompressed body.
    pub body: Vec<u8>,
}

/// Stores `body` (a no-op when its version exists) and makes it the active snapshot.
pub(crate) fn put(
    conn: &mut Connection,
    body: &[u8],
    fetched_at: Timestamp,
    source: Option<&str>,
) -> Result<PricingVersion, StoreError> {
    let version = PricingVersion::of_body(body);
    let compressed = zstd::encode_all(body, ZSTD_LEVEL)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT OR IGNORE INTO pricing_snapshot (version, fetched_at_us, source, body_zstd, body_len) \
         VALUES (:version, :fetched_at_us, :source, :body_zstd, :body_len)",
        named_params! {
            ":version": version.as_str(),
            ":fetched_at_us": to_us(fetched_at),
            ":source": source,
            ":body_zstd": compressed,
            ":body_len": to_i64(body.len() as u64, "body_len")?,
        },
    )?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('pricing_version', ?1) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [version.as_str()],
    )?;
    tx.commit()?;
    Ok(version)
}

/// The snapshot `meta.pricing_version` points at, if one was ever stored.
pub(crate) fn active(conn: &Connection) -> Result<Option<StoredPricingSnapshot>, StoreError> {
    conn.prepare_cached(
        "SELECT p.version, p.fetched_at_us, p.source, p.body_zstd, p.body_len \
         FROM pricing_snapshot p JOIN meta m ON m.key = 'pricing_version' AND m.value = p.version",
    )?
    .query_and_then([], |row| -> Result<StoredPricingSnapshot, StoreError> {
        let compressed: Vec<u8> = row.get("body_zstd")?;
        let body = zstd::decode_all(compressed.as_slice())?;
        let expected = to_u64(row.get("body_len")?, "body_len")?;
        if body.len() as u64 != expected {
            return Err(CodecError::Value {
                column: "body_len",
                value: format!("{expected} != {}", body.len()),
            }
            .into());
        }
        let version: String = row.get("version")?;
        Ok(StoredPricingSnapshot {
            version: PricingVersion::try_from(version).map_err(CodecError::from)?,
            fetched_at: row.ts("fetched_at_us")?,
            source: row.get("source")?,
            body,
        })
    })?
    .next()
    .transpose()
}
