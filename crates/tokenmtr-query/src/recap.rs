//! Menu-bar recap, budgets and the `status` document.

use jiff::{SignedDuration, Span, Timestamp};
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::Serialize;
use tokenmtr_core::{
    AgentId, Budget, BudgetMeasure, BudgetPeriod, Currency, DedupKey, EventCost, EventId,
    EventKind, ModelId, Money, ProfileId, ProjectKey, SessionId, TokenCounts, UsageEvent,
    UsdMicros, budget_status, live_rate, project_period,
};
use tokenmtr_store::StoreReader;

use crate::breakdowns::{Totals, models, overview};
use crate::context::QueryContext;
use crate::engine::{Dim, Scope, fetch};
use crate::error::{FilterError, QueryError};
use crate::filters::{ClientName, Filters};
use crate::health::{DaemonHealth, DaemonState, SinkHealth, SinkSpec, daemon_health, sink_health};
use crate::metrics_health::{MetricsHealth, MetricsSpec, metrics_health};
use crate::quota::{QuotaMeter, quota_latest};
use crate::range::{Bucket, DateRange, Period};
use crate::series::{GroupBy, series};

/// What a budget applies to; `None` is "everything".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BudgetScope {
    /// Agents.
    pub agents: Option<Vec<AgentId>>,
    /// Profiles.
    pub profiles: Option<Vec<ProfileId>>,
    /// Projects.
    pub projects: Option<Vec<ProjectKey>>,
    /// Clients.
    pub clients: Option<Vec<ClientName>>,
}

/// A budget with its scope; the caller builds these from its config.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetSpec {
    /// The budget.
    pub budget: Budget,
    /// What it covers.
    pub scope: BudgetScope,
}

impl BudgetSpec {
    fn filters(&self, range: DateRange) -> Filters {
        let s = &self.scope;
        let mut f = Filters::new(range);
        if let Some(v) = &s.agents {
            f = f.agents(v.iter().copied());
        }
        if let Some(v) = &s.profiles {
            f = f.profiles(v.iter().cloned());
        }
        if let Some(v) = &s.projects {
            f = f.projects(v.iter().cloned());
        }
        if let Some(v) = &s.clients {
            f = f.clients(v.iter().cloned());
        }
        f
    }
}

/// Whether a budget's money could be measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetState {
    /// Spent and projected amounts are real.
    Measured,
    /// The budget's currency needs an FX rate and the store has none yet (before the first ECB
    /// fetch): only the limit and period are real; spent, projected and percents are zero.
    NoFxRate,
}

/// One budget meter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetView {
    /// Whether the money fields below are measured.
    pub state: BudgetState,
    /// Budget id.
    pub id: String,
    /// Calendar period.
    pub period: BudgetPeriod,
    /// What is measured.
    pub measure: BudgetMeasure,
    /// Currency of the money fields (the budget's own).
    pub currency: Currency,
    /// First local day of the current period.
    pub period_start: jiff::civil::Date,
    /// First local day after the period.
    pub period_end: jiff::civil::Date,
    /// The limit.
    pub limit_micros: i64,
    /// Spent so far.
    pub spent_micros: i64,
    /// Whole percent of the limit spent.
    pub percent: u32,
    /// Projected spend at the end of the period.
    pub projected_micros: i64,
    /// Whole percent of the limit projected.
    pub projected_percent: u32,
}

const HISTORY_DAYS: i64 = 7;

fn period_of(period: BudgetPeriod) -> Period {
    match period {
        BudgetPeriod::Day => Period::Today,
        BudgetPeriod::Week => Period::Week,
        BudgetPeriod::Month => Period::Month,
    }
}

fn history_range(today: jiff::civil::Date) -> Result<DateRange, QueryError> {
    DateRange::new(today.checked_sub(Span::new().days(HISTORY_DAYS))?, today)
        .map_err(|_| QueryError::decode("range", today))
}

fn remaining_days(range: DateRange, today: jiff::civil::Date) -> u32 {
    range
        .end()
        .since(today)
        .ok()
        .and_then(|s| u32::try_from(s.get_days()).ok())
        .map_or(0, |d| d.saturating_sub(1))
}

/// Status of every budget for its current calendar period in the context's zone. Money is
/// converted to the budget's currency with the ECB rate of each day; a budget that needs an FX
/// rate when the store has none is listed as [`BudgetState::NoFxRate`].
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn budgets(
    reader: &StoreReader,
    ctx: &QueryContext,
    specs: &[BudgetSpec],
) -> Result<Vec<BudgetView>, QueryError> {
    let today = ctx.today();
    let mut out = Vec::new();
    for spec in specs {
        let range = period_of(spec.budget.period())
            .containing(today, ctx.week_start())
            .map_err(|_| QueryError::decode("budget period", today))?;
        match one_budget(reader, ctx, spec, range, today) {
            Ok(view) => out.push(view),
            Err(QueryError::NoFxRate { .. }) => out.push(unmeasured(spec, range)),
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn one_budget(
    reader: &StoreReader,
    ctx: &QueryContext,
    spec: &BudgetSpec,
    range: DateRange,
    today: jiff::civil::Date,
) -> Result<BudgetView, QueryError> {
    let budget = &spec.budget;
    let currency = budget.amount().currency();
    let local = ctx.in_currency(currency);
    let measure = |usage: &crate::usage::Usage| match budget.measure() {
        BudgetMeasure::Paid => usage.paid_micros,
        BudgetMeasure::ApiValue => usage.api_value_micros,
    };
    let spent = {
        let fetched = fetch(reader, &local, &spec.filters(range), Dim::None, Scope::All)?;
        let mut usage = crate::usage::Usage::default();
        fetched.rows.iter().for_each(|r| usage.add(r));
        fetched.idle.iter().for_each(|i| usage.add_idle(i));
        measure(&usage)
    };
    let history = series(
        reader,
        &local,
        &spec.filters(history_range(today)?),
        Bucket::Day,
        GroupBy::None,
    )?
    .points
    .iter()
    .map(|p| Money::from_micros(currency, measure(&p.usage)))
    .collect::<Vec<_>>();
    let status = budget_status(
        budget,
        Money::from_micros(currency, spent),
        &history,
        remaining_days(range, today),
    )?;
    Ok(BudgetView {
        state: BudgetState::Measured,
        id: budget.id().as_str().to_owned(),
        period: budget.period(),
        measure: budget.measure(),
        currency,
        period_start: range.start(),
        period_end: range.end(),
        limit_micros: status.limit.micros(),
        spent_micros: status.spent.micros(),
        percent: status.percent,
        projected_micros: status.projected.micros(),
        projected_percent: status.projected_percent,
    })
}

fn unmeasured(spec: &BudgetSpec, range: DateRange) -> BudgetView {
    let budget = &spec.budget;
    BudgetView {
        state: BudgetState::NoFxRate,
        id: budget.id().as_str().to_owned(),
        period: budget.period(),
        measure: budget.measure(),
        currency: budget.amount().currency(),
        period_start: range.start(),
        period_end: range.end(),
        limit_micros: budget.amount().micros(),
        spent_micros: 0,
        percent: 0,
        projected_micros: 0,
        projected_percent: 0,
    }
}

/// Everything the recap needs from the user's config.
#[derive(Debug, Clone, PartialEq)]
pub struct RecapConfig {
    /// Budgets.
    pub budgets: Vec<BudgetSpec>,
    /// Sinks.
    pub sinks: Vec<SinkSpec>,
    /// Metrics outputs.
    pub metrics: Vec<MetricsSpec>,
    /// Window of the live rate.
    pub live_window: SignedDuration,
    /// Heartbeat age after which the daemon counts as stale.
    pub daemon_stale_after: SignedDuration,
}

/// Which agents a recap covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Every agent, summed (the `All` tab).
    All,
    /// One agent.
    Agent(AgentId),
}

/// Current burn rate, present only while the window has events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Live {
    /// Processed tokens per minute.
    pub tokens_per_minute: u64,
    /// API value per hour, USD micros.
    pub api_value_per_hour_usd_micros: i64,
    /// Sessions with events in the window.
    pub active_sessions: u32,
}

/// One day of the sparkline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SparkPoint {
    /// Local day.
    pub date: jiff::civil::Date,
    /// API value in the recap currency.
    pub api_value_micros: i64,
}

/// One agent's (or all agents') recap for one period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentRecap {
    /// The agent; absent for the `All` tab.
    pub agent: Option<AgentId>,
    /// Period shown.
    pub period: Period,
    /// The period as local days.
    pub range: DateRange,
    /// Currency of the money fields (budgets use their own).
    pub currency: Currency,
    /// True when EUR was requested without FX rates.
    pub currency_fallback: bool,
    /// KPIs of the period; `paid_micros`, `api_value_micros`, `roi`, tokens, cached-input %.
    pub totals: Totals,
    /// Projected paid cost at the end of the period.
    pub projected_paid_micros: i64,
    /// Latest quota polls, with ETA.
    pub quota: Vec<QuotaMeter>,
    /// Budgets that apply.
    pub budgets: Vec<BudgetView>,
    /// API value of the last seven days, today included.
    pub sparkline: Vec<SparkPoint>,
    /// Live rate, absent when the window has no events.
    pub live: Option<Live>,
    /// Model with the biggest API value in the period.
    pub top_model: Option<String>,
    /// Latest event ever recorded.
    pub last_activity: Option<Timestamp>,
}

/// How bad things are: the worst of daemon, sinks and metrics outputs (the menu-bar status dot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthLevel {
    /// All fine (disabled sinks and a never-started daemon count as fine).
    Ok,
    /// Stale daemon, lagging sink or failing metrics output.
    Warning,
    /// A failing sink.
    Error,
}

/// Daemon, sink and metrics output state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemHealth {
    /// Worst of the below.
    pub level: HealthLevel,
    /// Daemon.
    pub daemon: DaemonHealth,
    /// Sinks.
    pub sinks: Vec<SinkHealth>,
    /// Metrics outputs; empty when none is configured.
    pub metrics: Vec<MetricsHealth>,
}

fn system_health(
    reader: &StoreReader,
    ctx: &QueryContext,
    cfg: &RecapConfig,
) -> Result<SystemHealth, QueryError> {
    let daemon = daemon_health(reader, ctx, cfg.daemon_stale_after)?;
    let sinks = sink_health(reader, ctx, &cfg.sinks)?;
    let metrics = metrics_health(reader, &cfg.metrics)?;
    let sink_level = match sinks.iter().map(|s| s.state.severity()).max() {
        Some(2) => HealthLevel::Error,
        Some(1) => HealthLevel::Warning,
        _ => HealthLevel::Ok,
    };
    let metrics_level = if metrics.iter().any(|m| m.state.severity() > 0) {
        HealthLevel::Warning
    } else {
        HealthLevel::Ok
    };
    let daemon_level = if daemon.state == DaemonState::Stale {
        HealthLevel::Warning
    } else {
        HealthLevel::Ok
    };
    Ok(SystemHealth {
        level: sink_level.max(daemon_level).max(metrics_level),
        daemon,
        sinks,
        metrics,
    })
}

fn live_events(
    reader: &StoreReader,
    now: Timestamp,
    window: SignedDuration,
) -> Result<Vec<UsageEvent>, QueryError> {
    let since = now.checked_sub(window)?;
    let mut statement = reader.connection().prepare_cached(
        "SELECT agent, session, at_us, input_uncached, cache_read, cache_write_5m, \
         cache_write_1h, output, reasoning, cost_api_usd_micros FROM usage_event \
         WHERE at_us > ? AND at_us <= ? AND kind NOT IN ('daily_aggregate', 'extra_usage')",
    )?;
    let mut rows = statement.query([since.as_microsecond(), now.as_microsecond()])?;
    let (Ok(dedup_key), Ok(model), Ok(_)) = (
        DedupKey::try_from("live"),
        ModelId::try_from("live"),
        SessionId::try_from("live"),
    ) else {
        return Ok(Vec::new());
    };
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let agent: String = row.get(0)?;
        let session: String = row.get(1)?;
        let at: i64 = row.get(2)?;
        let n = |i| -> Result<u64, QueryError> {
            let v: i64 = row.get(i)?;
            u64::try_from(v).map_err(|_| QueryError::decode("token count", v))
        };
        let cost: Option<i64> = row.get(9)?;
        events.push(UsageEvent {
            id: EventId::from_bytes([0; 16]),
            agent: agent
                .parse()
                .map_err(|_| QueryError::decode("agent", &agent))?,
            profile: ProfileId::default_profile(),
            dedup_key: dedup_key.clone(),
            at: Timestamp::from_microsecond(at).map_err(|_| QueryError::decode("timestamp", at))?,
            session: SessionId::try_from(session.clone())
                .map_err(|_| QueryError::decode("session", session))?,
            parent_session: None,
            subagent: None,
            is_sidechain: false,
            cwd: None,
            git_branch: None,
            project: None,
            client: None,
            source_file: None,
            approximate: false,
            provider: None,
            model: model.clone(),
            kind: EventKind::Message,
            tokens: TokenCounts {
                input_uncached: n(3)?,
                cache_read: n(4)?,
                cache_write_5m: n(5)?,
                cache_write_1h: n(6)?,
                output: n(7)?,
                reasoning: n(8)?,
            },
            cost: EventCost::new(cost.map(UsdMicros::new), None),
            service_tier: None,
        });
    }
    Ok(events)
}

fn live(
    reader: &StoreReader,
    ctx: &QueryContext,
    cfg: &RecapConfig,
    selection: Selection,
) -> Result<Option<Live>, QueryError> {
    let rates = live_rate(
        &live_events(reader, ctx.now(), cfg.live_window)?,
        ctx.now(),
        cfg.live_window,
    );
    let mut total: Option<Live> = None;
    for rate in rates
        .into_iter()
        .filter(|r| selection == Selection::All || selection == Selection::Agent(r.agent))
    {
        let acc = total.get_or_insert(Live {
            tokens_per_minute: 0,
            api_value_per_hour_usd_micros: 0,
            active_sessions: 0,
        });
        acc.tokens_per_minute += rate.tokens_per_minute;
        acc.api_value_per_hour_usd_micros += rate.api_value_per_hour.get();
        acc.active_sessions += rate.active_sessions;
    }
    Ok(total)
}

/// The newest live event; walks `usage_event_at` backwards to the first live row. Extra usage
/// rows sit at local noon of the poll day, possibly ahead of now: they are spend, not activity.
pub(crate) const LAST_ACTIVITY: &str =
    "SELECT MAX(at_us) FROM usage_event WHERE superseded = 0 AND kind <> 'extra_usage'";

/// The newest live event of agent `?1`. `agent = ?` alone picks the `(agent, dedup_key)` index
/// and reads every row of the agent. Instead, this skips from profile to profile on
/// `usage_event_agent_profile_at` and takes each profile's newest live row.
pub(crate) const LAST_ACTIVITY_OF_AGENT: &str = "WITH RECURSIVE p(profile) AS ( \
       SELECT MIN(profile) FROM usage_event WHERE agent = ?1 \
       UNION ALL \
       SELECT (SELECT MIN(profile) FROM usage_event WHERE agent = ?1 AND profile > p.profile) \
       FROM p WHERE p.profile IS NOT NULL) \
     SELECT MAX((SELECT MAX(at_us) FROM usage_event \
                 WHERE agent = ?1 AND profile = p.profile AND superseded = 0 \
                   AND kind <> 'extra_usage')) \
     FROM p WHERE p.profile IS NOT NULL";

fn last_activity(
    reader: &StoreReader,
    selection: Selection,
) -> Result<Option<Timestamp>, QueryError> {
    let (sql, params) = match selection {
        Selection::All => (LAST_ACTIVITY, Vec::new()),
        Selection::Agent(agent) => (
            LAST_ACTIVITY_OF_AGENT,
            vec![Value::Text(agent.as_str().to_owned())],
        ),
    };
    let at: Option<i64> = reader
        .connection()
        .prepare_cached(sql)?
        .query_row(params_from_iter(params), |row| row.get(0))?;
    at.map(|us| Timestamp::from_microsecond(us).map_err(|_| QueryError::decode("timestamp", us)))
        .transpose()
}

fn agent_recap(
    reader: &StoreReader,
    ctx: &QueryContext,
    cfg: &RecapConfig,
    selection: Selection,
    period: Period,
) -> Result<AgentRecap, QueryError> {
    let today = ctx.today();
    let range = period
        .containing(today, ctx.week_start())
        .map_err(|e: FilterError| QueryError::decode("period", e))?;
    let base = match selection {
        Selection::All => Filters::new(range),
        Selection::Agent(agent) => Filters::new(range).agents([agent]),
    };
    let totals = overview(reader, ctx, &base)?;
    let history = series(
        reader,
        ctx,
        &base.clone().with_range(history_range(today)?),
        Bucket::Day,
        GroupBy::None,
    )?;
    let paid_history: Vec<Money> = history
        .points
        .iter()
        .map(|p| Money::from_micros(ctx.currency(), p.usage.paid_micros))
        .collect();
    let projected = project_period(
        Money::from_micros(ctx.currency(), totals.usage.paid_micros),
        &paid_history,
        remaining_days(range, today),
    )?
    .micros();
    let spark_range = DateRange::new(
        today.checked_sub(Span::new().days(HISTORY_DAYS - 1))?,
        today.tomorrow()?,
    )
    .map_err(|_| QueryError::decode("range", today))?;
    let sparkline = series(
        reader,
        ctx,
        &base.clone().with_range(spark_range),
        Bucket::Day,
        GroupBy::None,
    )?
    .points
    .iter()
    .map(|p| SparkPoint {
        date: p.start,
        api_value_micros: p.usage.api_value_micros,
    })
    .collect();
    let mut meters_filter = Filters::new(range);
    if let Selection::Agent(agent) = selection {
        meters_filter = meters_filter.agents([agent]);
    }
    let applies = |spec: &&BudgetSpec| match (selection, &spec.scope.agents) {
        (Selection::Agent(agent), Some(agents)) => agents.contains(&agent),
        _ => true,
    };
    let applicable: Vec<BudgetSpec> = cfg.budgets.iter().filter(applies).cloned().collect();
    Ok(AgentRecap {
        agent: match selection {
            Selection::All => None,
            Selection::Agent(agent) => Some(agent),
        },
        period,
        range,
        currency: ctx.currency(),
        currency_fallback: ctx.currency_fallback(),
        top_model: models(reader, ctx, &base)?
            .rows
            .into_iter()
            .next()
            .map(|m| m.model),
        projected_paid_micros: projected,
        quota: quota_latest(reader, ctx, &meters_filter)?,
        budgets: budgets(reader, ctx, &applicable)?,
        sparkline,
        live: live(reader, ctx, cfg, selection)?,
        last_activity: last_activity(reader, selection)?,
        totals,
    })
}

/// The recap of one tab: `agent` (or all) for `period`, plus system health (also served by
/// `recap --json`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recap {
    /// Always 1.
    pub status_schema: u32,
    /// Query clock.
    pub generated_at: Timestamp,
    /// Daemon and sinks.
    pub system: SystemHealth,
    /// The tab.
    #[serde(flatten)]
    pub recap: AgentRecap,
}

/// Version of the `status` and `recap` JSON documents.
pub const STATUS_SCHEMA: u32 = 1;

/// Recap of one tab.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn recap(
    reader: &StoreReader,
    ctx: &QueryContext,
    cfg: &RecapConfig,
    selection: Selection,
    period: Period,
) -> Result<Recap, QueryError> {
    Ok(Recap {
        status_schema: STATUS_SCHEMA,
        generated_at: ctx.now(),
        system: system_health(reader, ctx, cfg)?,
        recap: agent_recap(reader, ctx, cfg, selection, period)?,
    })
}

/// The `status --json` document: system health and one recap per followed agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// Always 1.
    pub status_schema: u32,
    /// Query clock.
    pub generated_at: Timestamp,
    /// Daemon and sinks.
    pub system: SystemHealth,
    /// One recap per followed agent, in the given order.
    pub agents: Vec<AgentRecap>,
}

/// Status of `agents` (the followed ones) for `period`.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn status(
    reader: &StoreReader,
    ctx: &QueryContext,
    cfg: &RecapConfig,
    agents: &[AgentId],
    period: Period,
) -> Result<Status, QueryError> {
    Ok(Status {
        status_schema: STATUS_SCHEMA,
        generated_at: ctx.now(),
        system: system_health(reader, ctx, cfg)?,
        agents: agents
            .iter()
            .map(|a| agent_recap(reader, ctx, cfg, Selection::Agent(*a), period))
            .collect::<Result<_, _>>()?,
    })
}
