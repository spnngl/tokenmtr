//! The `fx_rate` table: EUR per USD by publication date (D27).

use std::sync::LazyLock;

use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{Connection, Row, named_params, params};
use serde::{Deserialize, Serialize};
use tokenmtr_core::{EurPerUsd, FxRate};

use crate::codec::{RowExt, enum_text, to_limit};
use crate::error::StoreError;
use crate::types::{Dataset, Revision, UpsertStats, Versioned};
use crate::versioned::{self, Merge, UpsertSql};

/// Where a rate came from; the `source` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxSource {
    /// The European Central Bank reference rates.
    Ecb,
}

/// A stored rate and its provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRateRow {
    /// The rate and its date.
    pub rate: FxRate,
    /// Where it came from.
    pub source: FxSource,
}

const VALUES: [(&str, Merge); 2] = [
    ("eur_per_usd_scaled", Merge::Replace),
    ("source", Merge::Replace),
];

static UPSERT: LazyLock<UpsertSql> =
    LazyLock::new(|| UpsertSql::new("fx_rate", &["date"], &["date"], &VALUES));

const AFTER: &str = "SELECT revision, date, eur_per_usd_scaled, source \
    FROM fx_rate WHERE revision > ?1 ORDER BY revision LIMIT ?2";

/// Inserts or updates rates by date.
pub(crate) fn upsert(
    conn: &Connection,
    now: Timestamp,
    source: FxSource,
    rates: &[FxRate],
) -> Result<UpsertStats, StoreError> {
    let rows = versioned::last_wins(rates, |r| r.date);
    let source = enum_text(&source)?;
    versioned::run(
        conn,
        Dataset::FxRates,
        &UPSERT,
        now,
        &rows,
        |upsert, rate, revision, now_us| {
            upsert.bind(named_params! {
                ":date": rate.date.to_string(),
                ":revision": revision,
                ":updated_at_us": now_us,
                ":eur_per_usd_scaled": rate.eur_per_usd.scaled(),
                ":source": source,
            })
        },
    )
}

/// Rates with a revision above `after`, oldest revision first.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<FxRateRow>>, StoreError> {
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, to_limit(limit)?], decode)?
        .collect()
}

fn decode(row: &Row<'_>) -> Result<Versioned<FxRateRow>, StoreError> {
    let date: String = row.get("date")?;
    Ok(Versioned {
        revision: row.get("revision")?,
        row: FxRateRow {
            rate: FxRate {
                date: date.parse::<Date>()?,
                eur_per_usd: EurPerUsd::from_scaled(row.get("eur_per_usd_scaled")?)?,
            },
            source: row.enumeration("source")?,
        },
    })
}
