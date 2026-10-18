//! The `account` table: identity and detected plan per agent profile (D26).

use jiff::Timestamp;
use rusqlite::{Connection, Row, named_params};
use tokenmtr_core::{AgentId, Plan, ProfileId};

use crate::codec::{RowExt, enum_text, to_us};
use crate::error::StoreError;

/// What an agent's credentials say about who is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// The agent.
    pub agent: AgentId,
    /// Its profile.
    pub profile: ProfileId,
    /// Account email.
    pub email: Option<String>,
    /// Organization name.
    pub organization: Option<String>,
    /// The subscription plan the provider reports.
    pub detected_plan: Option<Plan>,
    /// The provider's rate-limit tier, verbatim.
    pub rate_limit_tier: Option<String>,
    /// When this was fetched.
    pub fetched_at: Timestamp,
}

const SAVE: &str = "INSERT INTO account \
    (agent, profile, email, organization, detected_plan, rate_limit_tier, fetched_at_us) \
    VALUES (:agent, :profile, :email, :organization, :detected_plan, :rate_limit_tier, :fetched_at_us) \
    ON CONFLICT (agent, profile) DO UPDATE SET email = excluded.email, \
    organization = excluded.organization, detected_plan = excluded.detected_plan, \
    rate_limit_tier = excluded.rate_limit_tier, fetched_at_us = excluded.fetched_at_us";

/// Inserts or replaces the account of `(agent, profile)`.
pub(crate) fn save(conn: &Connection, account: &AccountRecord) -> Result<(), StoreError> {
    conn.prepare_cached(SAVE)?.execute(named_params! {
        ":agent": account.agent.as_str(),
        ":profile": account.profile.as_str(),
        ":email": account.email,
        ":organization": account.organization,
        ":detected_plan": account.detected_plan.as_ref().map(enum_text).transpose()?,
        ":rate_limit_tier": account.rate_limit_tier,
        ":fetched_at_us": to_us(account.fetched_at),
    })?;
    Ok(())
}

/// Every known account, ordered by agent and profile.
pub(crate) fn all(conn: &Connection) -> Result<Vec<AccountRecord>, StoreError> {
    conn.prepare_cached(
        "SELECT agent, profile, email, organization, detected_plan, rate_limit_tier, fetched_at_us \
         FROM account ORDER BY agent, profile",
    )?
    .query_and_then([], decode)?
    .collect()
}

fn decode(row: &Row<'_>) -> Result<AccountRecord, StoreError> {
    Ok(AccountRecord {
        agent: row.id::<AgentId>("agent")?,
        profile: row.id::<ProfileId>("profile")?,
        email: row.get("email")?,
        organization: row.get("organization")?,
        detected_plan: row.enumeration_opt("detected_plan")?,
        rate_limit_tier: row.get("rate_limit_tier")?,
        fetched_at: row.ts("fetched_at_us")?,
    })
}
