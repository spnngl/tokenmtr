//! The `subscription` table: subscriptions mirrored from the config (D28).
//!
//! Rows are never deleted, so a subscription removed from the config stays in the store (and in
//! exports) with its last known dates.

use std::sync::LazyLock;

use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};
use tokenmtr_core::{
    AgentId, Currency, Money, Plan, ProfileId, ProviderId, Subscription, SubscriptionId,
    SubscriptionSpec,
};

use crate::codec::{RowExt, enum_text, to_limit};
use crate::error::{CodecError, StoreError};
use crate::types::{Dataset, Revision, UpsertStats, Versioned};
use crate::versioned::{self, Merge};

const VALUES: [(&str, Merge); 9] = [
    ("provider", Merge::Replace),
    ("plan", Merge::Replace),
    ("agents", Merge::Replace),
    ("profiles", Merge::Replace),
    ("fee_micros", Merge::Replace),
    ("fee_currency", Merge::Replace),
    ("period", Merge::Replace),
    ("start_date", Merge::Replace),
    ("end_date", Merge::Replace),
];

static UPSERT: LazyLock<String> = LazyLock::new(|| {
    versioned::upsert_sql(
        "subscription",
        &["subscription_id"],
        &["subscription_id"],
        &VALUES,
    )
});

const AFTER: &str = "SELECT revision, subscription_id, provider, plan, agents, profiles, \
    fee_micros, fee_currency, period, start_date, end_date \
    FROM subscription WHERE revision > ?1 ORDER BY revision LIMIT ?2";

/// Inserts or updates subscriptions by id.
pub(crate) fn upsert(
    conn: &mut Connection,
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
        |statement, sub, revision, now_us| {
            let profiles = sub.profiles().map(serde_json::to_string).transpose()?;
            Ok(statement
                .query_row(
                    named_params! {
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
                    },
                    |r| r.get::<_, i64>(0),
                )
                .optional()?)
        },
    )
}

/// Subscriptions with a revision above `after`, oldest revision first.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<Subscription>>, StoreError> {
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, to_limit(limit)?], decode)?
        .collect()
}

fn date(row: &Row<'_>, column: &'static str) -> Result<Date, StoreError> {
    let text: String = row.get(column)?;
    Ok(text.parse::<Date>()?)
}

fn decode(row: &Row<'_>) -> Result<Versioned<Subscription>, StoreError> {
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
    Ok(Versioned {
        revision: row.get("revision")?,
        row: subscription,
    })
}
