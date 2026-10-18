//! The `subscription` table: subscriptions mirrored from the config (D28).
//!
//! Rows are never deleted: a subscription removed from the config stays in the store (and in
//! exports) with its last known dates and `removed_at_us`, and billing ignores it.

use std::collections::HashSet;
use std::sync::LazyLock;

use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{Connection, Row, named_params, params};
use tokenmtr_core::{
    AgentId, Currency, Money, Plan, ProfileId, ProviderId, Subscription, SubscriptionId,
    SubscriptionSpec,
};

use crate::codec::{RowExt, enum_text, from_us, to_count, to_limit, to_us};
use crate::error::{CodecError, StoreError};
use crate::types::{Dataset, Revision, StoredSubscription, UpsertStats, Versioned};
use crate::versioned::{self, Merge, UpsertSql};

const VALUES: [(&str, Merge); 10] = [
    ("provider", Merge::Replace),
    ("plan", Merge::Replace),
    ("agents", Merge::Replace),
    ("profiles", Merge::Replace),
    ("fee_micros", Merge::Replace),
    ("fee_currency", Merge::Replace),
    ("period", Merge::Replace),
    ("start_date", Merge::Replace),
    ("end_date", Merge::Replace),
    // Always bound `NULL`: a subscription back in the config is live again.
    ("removed_at_us", Merge::Replace),
];

static UPSERT: LazyLock<UpsertSql> = LazyLock::new(|| {
    UpsertSql::new(
        "subscription",
        &["subscription_id"],
        &["subscription_id"],
        &VALUES,
    )
});

/// The columns [`decode`] reads.
macro_rules! select {
    ($tail:literal) => {
        concat!(
            "SELECT revision, subscription_id, provider, plan, agents, profiles, fee_micros, \
             fee_currency, period, start_date, end_date, removed_at_us FROM subscription ",
            $tail
        )
    };
}

const AFTER: &str = select!("WHERE revision > ?1 ORDER BY revision LIMIT ?2");
const ACTIVE: &str = select!("WHERE removed_at_us IS NULL ORDER BY subscription_id");

/// Makes the table mirror `subscriptions` (D28): inserts or updates each by id, and tombstones
/// every live row whose id is absent with `removed_at_us = now`. Tombstoning and reviving bump
/// the revision so sinks see it.
pub(crate) fn mirror(
    conn: &Connection,
    now: Timestamp,
    subscriptions: &[Subscription],
) -> Result<UpsertStats, StoreError> {
    let mut stats = upsert(conn, now, subscriptions)?;
    let kept: HashSet<&str> = subscriptions.iter().map(|s| s.id().as_str()).collect();
    let removed: Vec<i64> = conn
        .prepare_cached("SELECT id, subscription_id FROM subscription WHERE removed_at_us IS NULL")?
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|row| match row {
            Ok((id, sub)) if !kept.contains(sub.as_str()) => Some(Ok(id)),
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<_, _>>()?;
    if removed.is_empty() {
        return Ok(stats);
    }
    let mut revision = versioned::head(conn, Dataset::Subscriptions)?;
    let mut tombstone = conn.prepare_cached(
        "UPDATE subscription SET removed_at_us = ?1, revision = ?2, updated_at_us = ?1 \
         WHERE id = ?3",
    )?;
    for id in &removed {
        revision = revision.next();
        tombstone.execute(params![to_us(now), revision, id])?;
    }
    conn.execute(
        "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
        params![revision, Dataset::Subscriptions.as_str()],
    )?;
    stats.updated += to_count(removed.len());
    Ok(stats)
}

fn upsert(
    conn: &Connection,
    now: Timestamp,
    subscriptions: &[Subscription],
) -> Result<UpsertStats, StoreError> {
    let rows = versioned::last_wins(subscriptions, |s| s.id().clone());
    versioned::run(
        conn,
        Dataset::Subscriptions,
        &UPSERT,
        now,
        &rows,
        |upsert, sub, revision, now_us| {
            let profiles = sub.profiles().map(serde_json::to_string).transpose()?;
            upsert.bind(named_params! {
                ":subscription_id": sub.id().as_str(),
                ":revision": revision,
                ":updated_at_us": now_us,
                ":provider": sub.provider().as_str(),
                ":plan": enum_text(&sub.plan())?,
                ":agents": serde_json::to_string(sub.agents())?,
                ":profiles": profiles,
                ":fee_micros": sub.fee().micros(),
                ":fee_currency": enum_text(&sub.fee().currency())?,
                ":period": enum_text(&sub.period())?,
                ":start_date": sub.start().to_string(),
                ":end_date": sub.end().map(|d| d.to_string()),
                ":removed_at_us": None::<i64>,
            })
        },
    )
}

/// Subscriptions with a revision above `after`, oldest revision first, removed ones included.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<StoredSubscription>>, StoreError> {
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, to_limit(limit)?], decode)?
        .collect()
}

/// Every subscription still in the config, by id.
pub(crate) fn active(conn: &Connection) -> Result<Vec<Subscription>, StoreError> {
    let mut statement = conn.prepare_cached(ACTIVE)?;
    statement
        .query_and_then([], |row| decode(row).map(|v| v.row.subscription))?
        .collect()
}

fn date(row: &Row<'_>, column: &'static str) -> Result<Date, StoreError> {
    let text: String = row.get(column)?;
    Ok(text.parse::<Date>()?)
}

fn decode(row: &Row<'_>) -> Result<Versioned<StoredSubscription>, StoreError> {
    let agents: String = row.get("agents")?;
    let profiles: Option<String> = row.get("profiles")?;
    let currency: Currency = row.enumeration("fee_currency")?;
    let end: Option<String> = row.get("end_date")?;
    let spec = SubscriptionSpec {
        id: row.id::<SubscriptionId>("subscription_id")?,
        provider: row.id::<ProviderId>("provider")?,
        plan: row.enumeration::<Plan>("plan")?,
        agents: serde_json::from_str::<Vec<AgentId>>(&agents)?,
        profiles: profiles
            .map(|p| serde_json::from_str::<Vec<ProfileId>>(&p))
            .transpose()?,
        fee: Money::from_micros(currency, row.get("fee_micros")?),
        period: row.enumeration("period")?,
        start: date(row, "start_date")?,
        end: end.map(|d| d.parse::<Date>()).transpose()?,
    };
    let subscription = Subscription::new(spec).map_err(CodecError::from)?;
    let removed_at: Option<i64> = row.get("removed_at_us")?;
    Ok(Versioned {
        revision: row.get("revision")?,
        row: StoredSubscription {
            subscription,
            removed_at: removed_at.map(from_us).transpose()?,
        },
    })
}
