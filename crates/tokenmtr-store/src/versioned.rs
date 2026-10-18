//! The upsert driver shared by the five exported datasets.
//!
//! Every exported row carries a `revision` taken from its dataset's counter (D17). A row gets a
//! new revision only when an upsert changes one of its value columns, so re-ingesting a file is a
//! no-op for sinks. The comparison is done by `SQLite` in the `ON CONFLICT ... WHERE` clause; the
//! counter advances once per changed row, so revisions are unique and gap-free.

use std::collections::HashSet;
use std::hash::Hash;

use jiff::Timestamp;
use rusqlite::{Connection, Statement, TransactionBehavior, params};

use crate::codec::to_us;
use crate::error::StoreError;
use crate::types::{Dataset, Revision, UpsertStats};

/// How an incoming value combines with the stored one.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Merge {
    /// The incoming value wins (D12).
    Replace,
    /// The incoming value wins unless it is `NULL`: for facts learned later that a re-read of an
    /// earlier line must not erase.
    KeepStored,
}

/// Builds `INSERT ... ON CONFLICT DO UPDATE ... WHERE <values differ> RETURNING id`.
///
/// Parameters are named after their columns (`:column`), plus `:revision` and `:updated_at_us`.
/// `immutable` columns are written on insert only; `conflict` is the unique key among them.
pub(crate) fn upsert_sql(
    table: &str,
    conflict: &[&str],
    immutable: &[&str],
    values: &[(&str, Merge)],
) -> String {
    let incoming = |column: &str, merge: Merge| match merge {
        Merge::Replace => format!("excluded.{column}"),
        Merge::KeepStored => format!("COALESCE(excluded.{column}, {table}.{column})"),
    };
    let columns: Vec<&str> = immutable
        .iter()
        .copied()
        .chain(values.iter().map(|(c, _)| *c))
        .chain(["revision", "updated_at_us"])
        .collect();
    let names = columns.join(", ");
    let params = columns
        .iter()
        .map(|c| format!(":{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let assignments = values
        .iter()
        .map(|(c, m)| format!("{c} = {}", incoming(c, *m)))
        .chain([
            "revision = excluded.revision".to_owned(),
            "updated_at_us = excluded.updated_at_us".to_owned(),
        ])
        .collect::<Vec<_>>()
        .join(", ");
    let stored = values
        .iter()
        .map(|(c, _)| format!("{table}.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let new = values
        .iter()
        .map(|(c, m)| incoming(c, *m))
        .collect::<Vec<_>>()
        .join(", ");
    let conflict = conflict.join(", ");
    format!(
        "INSERT INTO {table} ({names}) VALUES ({params}) \
         ON CONFLICT ({conflict}) DO UPDATE SET {assignments} \
         WHERE ({stored}) IS NOT ({new}) \
         RETURNING id"
    )
}

/// Keeps the last row per key, in order of last occurrence, so a batch repeating a key behaves as
/// if only its final row was sent (D12) and re-running the batch changes nothing.
pub(crate) fn last_wins<T, K: Hash + Eq>(rows: &[T], key: impl Fn(&T) -> K) -> Vec<&T> {
    let mut seen = HashSet::new();
    let mut kept: Vec<&T> = rows.iter().rev().filter(|r| seen.insert(key(r))).collect();
    kept.reverse();
    kept
}

/// Runs `bind` for each row inside one immediate transaction.
///
/// `bind` executes the [`upsert_sql`] statement with the given revision candidate and update
/// time (UTC micros) and returns the row id when the row was inserted or changed.
pub(crate) fn run<T>(
    conn: &mut Connection,
    dataset: Dataset,
    sql: &str,
    now: Timestamp,
    rows: &[T],
    mut bind: impl FnMut(&mut Statement<'_>, &T, Revision, i64) -> Result<Option<i64>, StoreError>,
) -> Result<UpsertStats, StoreError> {
    let mut stats = UpsertStats::default();
    if rows.is_empty() {
        return Ok(stats);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut revision: Revision = tx.query_row(
        "SELECT revision FROM dataset_revision WHERE dataset = ?1",
        [dataset.as_str()],
        |row| row.get(0),
    )?;
    // Rows are never deleted, so a row id above the current maximum is a new row.
    let max_id_before: i64 = tx.query_row(
        &format!("SELECT COALESCE(MAX(id), 0) FROM {}", dataset.table()),
        [],
        |row| row.get(0),
    )?;
    {
        let mut statement = tx.prepare_cached(sql)?;
        for row in rows {
            let candidate = revision.next();
            match bind(&mut statement, row, candidate, to_us(now))? {
                Some(id) => {
                    revision = candidate;
                    if id > max_id_before {
                        stats.inserted += 1;
                    } else {
                        stats.updated += 1;
                    }
                }
                None => stats.unchanged += 1,
            }
        }
    }
    if stats.changed() > 0 {
        tx.execute(
            "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
            params![revision, dataset.as_str()],
        )?;
    }
    tx.commit()?;
    Ok(stats)
}

/// The counter of a dataset: the revision of its newest change.
pub(crate) fn head(conn: &Connection, dataset: Dataset) -> Result<Revision, StoreError> {
    Ok(conn.query_row(
        "SELECT revision FROM dataset_revision WHERE dataset = ?1",
        [dataset.as_str()],
        |row| row.get(0),
    )?)
}

/// Rows past `after`: how many, and when the oldest was written (D41).
pub(crate) fn pending(
    conn: &Connection,
    dataset: Dataset,
    after: Revision,
) -> Result<crate::types::Pending, StoreError> {
    use crate::codec::{from_us, to_u64};
    let (rows, oldest): (i64, Option<i64>) = conn.query_row(
        &format!(
            "SELECT COUNT(*), MIN(updated_at_us) FROM {} WHERE revision > ?1",
            dataset.table()
        ),
        [after],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(crate::types::Pending {
        rows: to_u64(rows, "count").map_err(StoreError::from)?,
        oldest: oldest.map(from_us).transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_compares_stored_and_incoming_values() {
        let sql = upsert_sql(
            "t",
            &["k"],
            &["k"],
            &[("a", Merge::Replace), ("b", Merge::KeepStored)],
        );
        assert_eq!(
            sql,
            "INSERT INTO t (k, a, b, revision, updated_at_us) \
             VALUES (:k, :a, :b, :revision, :updated_at_us) \
             ON CONFLICT (k) DO UPDATE SET a = excluded.a, b = COALESCE(excluded.b, t.b), \
             revision = excluded.revision, updated_at_us = excluded.updated_at_us \
             WHERE (t.a, t.b) IS NOT (excluded.a, COALESCE(excluded.b, t.b)) \
             RETURNING id"
        );
    }

    #[test]
    fn last_row_per_key_wins_in_order_of_last_occurrence() {
        let rows = [(1, 'a'), (2, 'b'), (1, 'c')];
        let kept = last_wins(&rows, |r| r.0);
        assert_eq!(kept, vec![&(2, 'b'), &(1, 'c')]);
    }
}
