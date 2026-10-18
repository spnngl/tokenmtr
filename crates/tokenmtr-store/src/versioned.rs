//! The upsert driver shared by the five exported datasets.
//!
//! Every exported row carries a `revision` taken from its dataset's counter (D17). A row gets a
//! new revision only when an upsert changes one of its value columns, so re-ingesting a file is a
//! no-op for sinks. The comparison is done by `SQLite` in the `UPDATE ... WHERE` clause; the
//! counter advances once per changed row, so revisions are unique and gap-free.
//!
//! Each row is written by two statements, prepared once and cached: `INSERT ... ON CONFLICT DO
//! NOTHING`, and when that inserts nothing, `UPDATE ... WHERE <values differ>`. Neither has a
//! `RETURNING` clause. Both choices are measured (release, 5000 `usage_event` rows, one
//! transaction, 5 secondary indexes):
//!
//! - `RETURNING` stepped per row costs about 20x: 260 ms against 13 ms.
//! - A single `INSERT ... ON CONFLICT DO UPDATE SET source_file_id = ...` costs 280 ms for new
//!   rows however the rest is written, because `SQLite` re-checks the foreign key once per
//!   upserted row; the two-statement form takes 30 ms for new rows, 10 ms for unchanged ones.
//!
//! The number of rows each statement changed is all the driver needs.

use std::collections::HashSet;
use std::hash::Hash;

use jiff::Timestamp;
use rusqlite::types::ToSql;
use rusqlite::{CachedStatement, Connection, params};

use crate::codec::{to_count, to_u64, to_us};
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

/// The two statements that write one row: an insert and, when the key exists, an update.
///
/// Parameters are named after their columns (`:column`), plus `:revision` and `:updated_at_us`.
#[derive(Debug)]
pub(crate) struct UpsertSql {
    insert: String,
    update: String,
}

impl UpsertSql {
    /// `immutable` columns are written on insert only; `conflict` is the unique key, a subset of
    /// them. The update touches a row only when a value column differs from the stored one.
    pub(crate) fn new(
        table: &str,
        conflict: &[&str],
        immutable: &[&str],
        values: &[(&str, Merge)],
    ) -> Self {
        let incoming = |column: &str, merge: Merge| match merge {
            Merge::Replace => format!(":{column}"),
            Merge::KeepStored => format!("COALESCE(:{column}, {table}.{column})"),
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
                "revision = :revision".to_owned(),
                "updated_at_us = :updated_at_us".to_owned(),
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
        let key = conflict
            .iter()
            .map(|c| format!("{c} = :{c}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let conflict = conflict.join(", ");
        Self {
            insert: format!(
                "INSERT INTO {table} ({names}) VALUES ({params}) \
                 ON CONFLICT ({conflict}) DO NOTHING"
            ),
            update: format!(
                "UPDATE {table} SET {assignments} WHERE {key} AND ({stored}) IS NOT ({new})"
            ),
        }
    }
}

/// The prepared statements of one [`UpsertSql`].
pub(crate) struct Upsert<'c> {
    insert: CachedStatement<'c>,
    update: CachedStatement<'c>,
}

impl Upsert<'_> {
    /// Binds `params` to every statement that names them, so one `named_params!` list serves
    /// both.
    pub(crate) fn bind(&mut self, params: &[(&str, &dyn ToSql)]) -> Result<(), StoreError> {
        for statement in [&mut self.insert, &mut self.update] {
            for (name, value) in params {
                if let Some(index) = statement.parameter_index(name)? {
                    statement.raw_bind_parameter(index, value)?;
                }
            }
        }
        Ok(())
    }
}

/// Keeps the last row per key, in order of last occurrence, so a batch repeating a key behaves as
/// if only its final row was sent (D12) and re-running the batch changes nothing.
pub(crate) fn last_wins<'a, T, K: Hash + Eq>(
    rows: &'a [T],
    key: impl Fn(&'a T) -> K,
) -> Vec<&'a T> {
    let mut seen = HashSet::new();
    let mut kept: Vec<&T> = rows.iter().rev().filter(|r| seen.insert(key(r))).collect();
    kept.reverse();
    kept
}

/// Runs `bind` for each row on `conn`, which the caller has put inside a write transaction.
///
/// `bind` binds every parameter of `sql` for the row, with the given revision candidate and
/// update time (UTC micros). The row is inserted, else updated if a value differs, else left as
/// it is; only the first two consume the candidate.
pub(crate) fn run<T>(
    conn: &Connection,
    dataset: Dataset,
    sql: &UpsertSql,
    now: Timestamp,
    rows: &[T],
    mut bind: impl FnMut(&mut Upsert<'_>, &T, Revision, i64) -> Result<(), StoreError>,
) -> Result<UpsertStats, StoreError> {
    if rows.is_empty() {
        return Ok(UpsertStats::default());
    }
    let mut revision = head(conn, dataset)?;
    let now_us = to_us(now);
    let (mut inserted, mut updated) = (0_u64, 0_u64);
    {
        let mut statements = Upsert {
            insert: conn.prepare_cached(&sql.insert)?,
            update: conn.prepare_cached(&sql.update)?,
        };
        for row in rows {
            let candidate = revision.next();
            bind(&mut statements, row, candidate, now_us)?;
            if statements.insert.raw_execute()? > 0 {
                inserted += 1;
            } else if statements.update.raw_execute()? > 0 {
                updated += 1;
            } else {
                continue;
            }
            revision = candidate;
        }
    }
    let unchanged = to_count(rows.len()).saturating_sub(inserted + updated);
    if inserted + updated > 0 {
        conn.execute(
            "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
            params![revision, dataset.as_str()],
        )?;
    }
    Ok(UpsertStats {
        inserted,
        updated,
        unchanged,
    })
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
    use crate::codec::from_us;
    let (rows, oldest): (i64, Option<i64>) = conn.query_row(
        &format!(
            "SELECT COUNT(*), MIN(updated_at_us) FROM {} WHERE revision > ?1",
            crate::types::table(dataset)
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
        let sql = UpsertSql::new(
            "t",
            &["k"],
            &["k"],
            &[("a", Merge::Replace), ("b", Merge::KeepStored)],
        );
        assert_eq!(
            sql.insert,
            "INSERT INTO t (k, a, b, revision, updated_at_us) \
             VALUES (:k, :a, :b, :revision, :updated_at_us) ON CONFLICT (k) DO NOTHING"
        );
        assert_eq!(
            sql.update,
            "UPDATE t SET a = :a, b = COALESCE(:b, t.b), revision = :revision, \
             updated_at_us = :updated_at_us \
             WHERE k = :k AND (t.a, t.b) IS NOT (:a, COALESCE(:b, t.b))"
        );
    }

    #[test]
    fn last_row_per_key_wins_in_order_of_last_occurrence() {
        let rows = [(1, 'a'), (2, 'b'), (1, 'c')];
        let kept = last_wins(&rows, |r| r.0);
        assert_eq!(kept, vec![&(2, 'b'), &(1, 'c')]);
    }
}
