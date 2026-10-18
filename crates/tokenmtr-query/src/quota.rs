//! Quota meters, history and the plan advisor (D33, D35).

use std::collections::BTreeMap;

use jiff::{SignedDuration, Timestamp};
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::Serialize;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    Advice, AgentId, Plan, PlanCatalog, QuotaEta, QuotaSnapshot, QuotaWindow, UsdMicros,
    Utilization, WindowId, advise, quota_eta,
};
use tokenmtr_store::{StoreRead, StoreReader};

use crate::context::QueryContext;
use crate::engine::{Dim, Scope, fetch};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::range::DateRange;

fn micros_ts(us: i64) -> Result<Timestamp, QueryError> {
    Timestamp::from_microsecond(us).map_err(|_| QueryError::decode("timestamp", us))
}

fn parse_agent(text: &str) -> Result<AgentId, QueryError> {
    text.parse().map_err(|_| QueryError::decode("agent", text))
}

/// Successful polls of one agent profile in `[from, to)`, oldest first.
/// One `quota_snapshot` row: window id, used percent, reset µs, status, account.
type WindowRow = (String, Option<f64>, Option<i64>, String, Option<String>);

pub(crate) fn load_snapshots(
    reader: &StoreReader,
    agent: AgentId,
    profile: &str,
    from: Timestamp,
    to: Timestamp,
) -> Result<Vec<QuotaSnapshot>, QueryError> {
    let mut statement = reader.connection().prepare_cached(
        "SELECT fetched_at_us, window_id, used_percent, resets_at_us FROM quota_snapshot \
         WHERE agent = ? AND profile = ? AND status = 'ok' AND window_id <> '' \
         AND used_percent IS NOT NULL AND fetched_at_us >= ? AND fetched_at_us < ? \
         ORDER BY fetched_at_us, window_id",
    )?;
    let params = [
        Value::Text(agent.as_str().to_owned()),
        Value::Text(profile.to_owned()),
        Value::Integer(from.as_microsecond()),
        Value::Integer(to.as_microsecond()),
    ];
    let mut rows = statement.query(params_from_iter(params))?;
    let mut polls: BTreeMap<i64, Vec<QuotaWindow>> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let at: i64 = row.get(0)?;
        let id: String = row.get(1)?;
        let percent: f64 = row.get(2)?;
        let resets: Option<i64> = row.get(3)?;
        polls.entry(at).or_default().push(QuotaWindow {
            id: WindowId::new(id.clone()).map_err(|_| QueryError::decode("window", id))?,
            state: QuotaWindowState {
                utilization: Utilization::new(percent)
                    .map_err(|_| QueryError::decode("used_percent", percent))?,
                resets_at: resets.map(micros_ts).transpose()?,
            },
        });
    }
    polls
        .into_iter()
        .map(|(at, windows)| {
            Ok(QuotaSnapshot {
                fetched_at: micros_ts(at)?,
                windows,
            })
        })
        .collect()
}

/// One quota window of the latest poll.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaWindowView {
    /// Window id (`five_hour`, `seven_day`, …).
    pub window: String,
    /// Percent used; may exceed 100.
    pub used_percent: f64,
    /// When the window resets.
    pub resets_at: Option<Timestamp>,
    /// Seconds until the reset, never negative.
    pub resets_in_secs: Option<i64>,
    /// Whether the window will be exhausted before it resets (D33).
    pub eta: QuotaEta,
}

/// The latest poll of one agent profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaMeter {
    /// Agent.
    pub agent: AgentId,
    /// Profile.
    pub profile: String,
    /// When the poll ran.
    pub fetched_at: Timestamp,
    /// Seconds since the poll.
    pub age_secs: i64,
    /// `ok`, `signed_out`, `token_expired` or `failed`.
    pub status: String,
    /// Account the poll belongs to.
    pub account: Option<String>,
    /// Windows; empty unless `status` is `ok`.
    pub windows: Vec<QuotaWindowView>,
}

const ETA_LOOKBACK: SignedDuration = SignedDuration::from_hours(8 * 24);

/// The latest poll of every polled agent profile (restricted by the filters' agents/profiles),
/// with a per-window exhaustion ETA from the last eight days of polls.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn quota_latest(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Vec<QuotaMeter>, QueryError> {
    let mut sql =
        String::from("SELECT agent, profile, MAX(fetched_at_us) FROM quota_snapshot WHERE 1 = 1");
    let mut params = Vec::new();
    filters.sql_agent_profile(&mut sql, &mut params);
    sql.push_str(" GROUP BY agent, profile ORDER BY agent, profile");
    let latest: Vec<(String, String, i64)> = reader
        .connection()
        .prepare(&sql)?
        .query_map(params_from_iter(params), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut meters = Vec::new();
    for (agent_text, profile, at_us) in latest {
        let agent = parse_agent(&agent_text)?;
        let fetched_at = micros_ts(at_us)?;
        let mut statement = reader.connection().prepare_cached(
            "SELECT window_id, used_percent, resets_at_us, status, account FROM quota_snapshot \
             WHERE agent = ? AND profile = ? AND fetched_at_us = ? ORDER BY window_id",
        )?;
        let rows: Vec<WindowRow> = statement
            .query_map(
                [
                    Value::Text(agent_text.clone()),
                    Value::Text(profile.clone()),
                    Value::Integer(at_us),
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?
            .collect::<Result<_, _>>()?;
        let status = rows.first().map_or_else(String::new, |r| r.3.clone());
        let account = rows.iter().find_map(|r| r.4.clone());
        let history = load_snapshots(
            reader,
            agent,
            &profile,
            fetched_at.checked_sub(ETA_LOOKBACK)?,
            fetched_at.checked_add(SignedDuration::from_secs(1))?,
        )?;
        let mut windows = Vec::new();
        for (window, percent, resets, _, _) in rows {
            let (Some(used_percent), true) = (percent, !window.is_empty()) else {
                continue;
            };
            let resets_at = resets.map(micros_ts).transpose()?;
            let eta = match (&resets_at, WindowId::new(window.clone())) {
                (Some(at), Ok(id)) => quota_eta(&history, &id, *at),
                _ => QuotaEta::NotEnoughData,
            };
            windows.push(QuotaWindowView {
                window,
                used_percent,
                resets_in_secs: resets_at.map(|at| (at.as_second() - ctx.now().as_second()).max(0)),
                resets_at,
                eta,
            });
        }
        meters.push(QuotaMeter {
            agent,
            profile,
            fetched_at,
            age_secs: (ctx.now().as_second() - fetched_at.as_second()).max(0),
            status,
            account,
            windows,
        });
    }
    Ok(meters)
}

/// One sample of a quota history line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct QuotaPoint {
    /// Poll time.
    pub at: Timestamp,
    /// Percent used.
    pub used_percent: f64,
}

/// The history of one window of one agent profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaHistory {
    /// Agent.
    pub agent: AgentId,
    /// Profile.
    pub profile: String,
    /// Window id.
    pub window: String,
    /// Samples in time order.
    pub points: Vec<QuotaPoint>,
    /// Window resets inside the range (distinct reset times, 5-minute jitter merged).
    pub resets: Vec<Timestamp>,
}

/// Quota history of every polled window of `agent` in `range` (D33 `/quota`).
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn quota_history(
    reader: &StoreReader,
    ctx: &QueryContext,
    agent: AgentId,
    profile: &str,
    range: DateRange,
) -> Result<Vec<QuotaHistory>, QueryError> {
    let (from, to) = range.instants(ctx.zone())?;
    let snapshots = load_snapshots(reader, agent, profile, from, to)?;
    let mut by_window: BTreeMap<String, QuotaHistory> = BTreeMap::new();
    for snapshot in &snapshots {
        for window in &snapshot.windows {
            let entry = by_window
                .entry(window.id.as_str().to_owned())
                .or_insert_with(|| QuotaHistory {
                    agent,
                    profile: profile.to_owned(),
                    window: window.id.as_str().to_owned(),
                    points: Vec::new(),
                    resets: Vec::new(),
                });
            entry.points.push(QuotaPoint {
                at: snapshot.fetched_at,
                used_percent: window.state.utilization.percent(),
            });
            if let Some(reset) = window.state.resets_at
                && reset <= ctx.now()
                && reset >= from
                && entry
                    .resets
                    .last()
                    .is_none_or(|last| reset.as_second() - last.as_second() > 300)
            {
                entry.resets.push(reset);
            }
        }
    }
    Ok(by_window.into_values().collect())
}

/// The advisor's verdict and the inputs behind it (D35).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdvisorReport {
    /// Agent.
    pub agent: AgentId,
    /// Profile.
    pub profile: String,
    /// The plan assumed current; absent when unknown.
    pub current_plan: Option<Plan>,
    /// Quota polls of the last 30 days that fed the advisor.
    pub snapshots: usize,
    /// API value of the last 30 days, scaled to an average month, USD micros.
    pub monthly_api_value_usd_micros: i64,
    /// The verdict; `not_applicable` when the current plan is unknown.
    pub advice: Advice,
}

/// Runs the advisor over the last 30 days for one agent profile. `current` overrides the plan
/// detected from the account; without either the advice is `not_applicable`.
///
/// # Errors
///
/// Store, SQL or decode failures; a broken embedded plan catalog.
pub fn advisor(
    reader: &StoreReader,
    ctx: &QueryContext,
    agent: AgentId,
    profile: &tokenmtr_core::ProfileId,
    current: Option<Plan>,
    headroom: u8,
) -> Result<AdvisorReport, QueryError> {
    let today = ctx.today();
    let range = DateRange::new(
        today
            .checked_sub(jiff::Span::new().days(29))
            .map_err(QueryError::Time)?,
        today.tomorrow()?,
    )
    .map_err(|_| QueryError::decode("range", today))?;
    let (from, to) = range.instants(ctx.zone())?;
    let snapshots = load_snapshots(reader, agent, profile.as_str(), from, to)?;
    let plan = match current {
        Some(plan) => Some(plan),
        None => reader
            .accounts()?
            .into_iter()
            .find(|a| a.agent == agent && a.profile == *profile)
            .and_then(|a| a.detected_plan),
    };
    let filters = Filters::new(range)
        .agents([agent])
        .profiles([profile.clone()]);
    let value: i64 = fetch(reader, ctx, &filters, Dim::None, Scope::All)?
        .rows
        .iter()
        .map(|r| r.cell.cost_usd)
        .sum();
    // 30 days → average month (365.25 / 12 days).
    let monthly = i64::try_from(i128::from(value) * 30_437 / 30_000).unwrap_or(i64::MAX);
    let advice = match plan {
        Some(plan) => advise(
            &snapshots,
            &PlanCatalog::embedded()?,
            plan,
            headroom,
            UsdMicros::new(monthly),
        ),
        None => Advice::NotApplicable,
    };
    Ok(AdvisorReport {
        agent,
        profile: profile.as_str().to_owned(),
        current_plan: plan,
        snapshots: snapshots.len(),
        monthly_api_value_usd_micros: monthly,
        advice,
    })
}
