//! Session charts: per-session points, cost histogram, activity by day and hour, and the
//! timeline of one session.
//!
//! Like the sessions list, these views count real traffic only (no daily aggregates, no extra
//! usage) and split no subscription fee per session: `paid_micros` is the API value of
//! pay-as-you-go events.

use std::collections::{BTreeMap, HashMap};

use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::Serialize;
use tokenmtr_core::fx::utc_date;
use tokenmtr_core::{AgentId, Currency, ProfileId, ProviderId, SessionId, SignalKind, TokenCounts};
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{Dim, Scope, closed, covering, fetch, micros_ts, to_count, usd_to};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::usage::Usage;

/// Most session rows a [`SessionAnalytics`] carries, the most expensive first.
pub const SESSION_ROWS_LIMIT: usize = 500;

/// Lower edges of the cost histogram buckets in result-currency micros: 0, then 1-2-5 steps
/// from 0.01 to 100. The last bucket is open-ended.
pub const COST_EDGES_MICROS: [i64; 14] = [
    0,
    10_000,
    20_000,
    50_000,
    100_000,
    200_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
    10_000_000,
    20_000_000,
    50_000_000,
    100_000_000,
];

/// One session as a chart point.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionPoint {
    /// Agent.
    pub agent: AgentId,
    /// Session id.
    pub session: String,
    /// First event in the range.
    pub started_at: Timestamp,
    /// Seconds between its first and last event in the range.
    pub duration_secs: i64,
    /// API value in the result currency.
    pub api_value_micros: i64,
    /// Pay-as-you-go cost in the result currency (subscription fees are not split per session).
    pub paid_micros: i64,
    /// `cache_read / (input_uncached + cache_read + cache writes)` in percent; absent without
    /// input.
    pub cache_hit_percent: Option<f64>,
    /// Token mix.
    pub tokens: TokenCounts,
    /// Tool calls recorded for the session (whole session, not only the range).
    pub tool_calls: u64,
    /// Tool calls that failed.
    pub tool_errors: u64,
}

/// One bucket of the session cost histogram: API value in `[min_micros, max_micros)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CostBucket {
    /// Inclusive lower edge (the first bucket also holds negative values).
    pub min_micros: i64,
    /// Exclusive upper edge; absent for the last bucket.
    pub max_micros: Option<i64>,
    /// Sessions in the bucket.
    pub sessions: u64,
}

/// Sessions started on one local day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SessionDay {
    /// Local day.
    pub day: Date,
    /// Sessions whose first event in the range falls on it.
    pub sessions: u64,
}

/// Sessions started in one local weekday and hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SessionStartCell {
    /// 1 = Monday … 7 = Sunday, in the configured zone.
    pub weekday: u8,
    /// Local hour 0–23.
    pub hour: u8,
    /// Sessions started then.
    pub sessions: u64,
}

/// Session charts of a range.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionAnalytics {
    /// Currency of the money fields.
    pub currency: Currency,
    /// All sessions matching the filters; more than `rows` holds when it hit the cap.
    pub total: u64,
    /// At most [`SESSION_ROWS_LIMIT`] sessions, highest API value first.
    pub rows: Vec<SessionPoint>,
    /// Every session by API value, one bucket per [`COST_EDGES_MICROS`] edge.
    pub histogram: Vec<CostBucket>,
    /// Sessions per local day of the range, zero-filled.
    pub days: Vec<SessionDay>,
    /// Sessions by start weekday and hour: 168 cells, weekday-major.
    pub starts: Vec<SessionStartCell>,
}

struct Acc {
    first: Timestamp,
    last: Timestamp,
    usage: Usage,
}

/// Session charts of `filters`.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn session_analytics(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<SessionAnalytics, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::Session, Scope::Traffic)?;
    let mut by_session: BTreeMap<(AgentId, String), Acc> = BTreeMap::new();
    for row in &fetched.rows {
        let key = (row.cell.agent, row.cell.k1.clone().unwrap_or_default());
        let acc = by_session.entry(key).or_insert_with(|| Acc {
            first: row.cell.first_at,
            last: row.cell.last_at,
            usage: Usage::default(),
        });
        acc.first = acc.first.min(row.cell.first_at);
        acc.last = acc.last.max(row.cell.last_at);
        acc.usage.add(row);
    }
    let mut histogram: Vec<CostBucket> = COST_EDGES_MICROS
        .iter()
        .enumerate()
        .map(|(i, &min_micros)| CostBucket {
            min_micros,
            max_micros: COST_EDGES_MICROS.get(i + 1).copied(),
            sessions: 0,
        })
        .collect();
    let mut days: Vec<SessionDay> = filters
        .range()
        .dates()
        .map(|day| SessionDay { day, sessions: 0 })
        .collect();
    let mut starts: Vec<SessionStartCell> = (1..=7_u8)
        .flat_map(|weekday| {
            (0..24_u8).map(move |hour| SessionStartCell {
                weekday,
                hour,
                sessions: 0,
            })
        })
        .collect();
    for acc in by_session.values() {
        let bucket = COST_EDGES_MICROS
            .partition_point(|&edge| edge <= acc.usage.api_value_micros)
            .saturating_sub(1);
        if let Some(b) = histogram.get_mut(bucket) {
            b.sessions += 1;
        }
        let local = ctx.zone().to_datetime(acc.first);
        if let Some(d) = days.iter_mut().find(|d| d.day == local.date()) {
            d.sessions += 1;
        }
        let weekday = usize::try_from(local.weekday().to_monday_zero_offset()).unwrap_or(0);
        let hour = usize::try_from(local.hour()).unwrap_or(0);
        if let Some(cell) = starts.get_mut(weekday * 24 + hour) {
            cell.sessions += 1;
        }
    }
    Ok(SessionAnalytics {
        currency: ctx.currency(),
        total: u64::try_from(by_session.len()).unwrap_or(u64::MAX),
        rows: top_points(reader, by_session)?,
        histogram,
        days,
        starts,
    })
}

/// The [`SESSION_ROWS_LIMIT`] sessions of highest API value, with their tool counts.
fn top_points(
    reader: &StoreReader,
    by_session: BTreeMap<(AgentId, String), Acc>,
) -> Result<Vec<SessionPoint>, QueryError> {
    let mut ordered: Vec<_> = by_session.into_iter().collect();
    ordered.sort_by(|a, b| {
        b.1.usage
            .api_value_micros
            .cmp(&a.1.usage.api_value_micros)
            .then_with(|| a.0.cmp(&b.0))
    });
    ordered.truncate(SESSION_ROWS_LIMIT);
    let keys: Vec<&(AgentId, String)> = ordered.iter().map(|(key, _)| key).collect();
    let tools = tool_counts(reader, &keys)?;
    Ok(ordered
        .into_iter()
        .map(|((agent, session), acc)| {
            let (tool_calls, tool_errors) = tools
                .get(&(agent, session.clone()))
                .copied()
                .unwrap_or_default();
            let u = &acc.usage;
            SessionPoint {
                agent,
                session,
                started_at: acc.first,
                duration_secs: acc.last.as_second() - acc.first.as_second(),
                api_value_micros: u.api_value_micros,
                paid_micros: u.paid_micros,
                cache_hit_percent: u.cache_hit_percent(),
                tokens: TokenCounts {
                    input_uncached: u.input_uncached,
                    cache_read: u.cache_read,
                    cache_write_5m: u.cache_write_5m,
                    cache_write_1h: u.cache_write_1h,
                    output: u.output,
                    reasoning: u.reasoning,
                },
                tool_calls,
                tool_errors,
            }
        })
        .collect())
}

/// Calls and failed calls per (agent, session).
type ToolCounts = HashMap<(AgentId, String), (u64, u64)>;

/// Sessions per tool-count query: two parameters each, far below `SQLite`'s parameter limit.
const TOOL_CHUNK: usize = 250;

/// The tool-count query for `n` sessions.
pub(crate) fn tool_counts_sql(n: usize) -> String {
    format!(
        "WITH page(agent, session) AS (VALUES {}) SELECT p.agent, p.session, COUNT(t.id), \
         COALESCE(SUM(t.is_error = 1), 0) FROM page p \
         JOIN tool_call t ON t.session = p.session AND t.agent = p.agent \
         GROUP BY p.agent, p.session",
        vec!["(?, ?)"; n].join(", ")
    )
}

/// Tool calls and failed tool calls of `keys`; sessions without calls are absent.
fn tool_counts(
    reader: &StoreReader,
    keys: &[&(AgentId, String)],
) -> Result<ToolCounts, QueryError> {
    let mut out = HashMap::new();
    for chunk in keys.chunks(TOOL_CHUNK) {
        let params = chunk.iter().flat_map(|(agent, session)| {
            [
                Value::Text(agent.as_str().to_owned()),
                Value::Text(session.clone()),
            ]
        });
        let mut statement = reader
            .connection()
            .prepare_cached(&tool_counts_sql(chunk.len()))?;
        let mut rows = statement.query(params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            let agent: String = row.get(0)?;
            out.insert(
                (
                    agent
                        .parse()
                        .map_err(|_| QueryError::decode("agent", &agent))?,
                    row.get(1)?,
                ),
                (to_count(row.get(2)?)?, to_count(row.get(3)?)?),
            );
        }
    }
    Ok(out)
}

/// One usage event of a session timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineEvent {
    /// When.
    pub at: Timestamp,
    /// Model.
    pub model: String,
    /// Token mix.
    pub tokens: TokenCounts,
    /// API value in the result currency.
    pub api_value_micros: i64,
    /// Pay-as-you-go cost in the result currency (0 when a subscription covers the event).
    pub paid_micros: i64,
    /// Running API value since the session's first event.
    pub cumulative_api_value_micros: i64,
    /// Running paid cost since the session's first event.
    pub cumulative_paid_micros: i64,
}

/// A compaction or API error of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TimelineSignal {
    /// When.
    pub at: Timestamp,
    /// What.
    pub kind: SignalKind,
}

/// The calls of one tool in a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineTool {
    /// Tool name.
    pub tool_name: String,
    /// MCP server, when it is an MCP tool.
    pub mcp_server: Option<String>,
    /// Calls.
    pub calls: u64,
    /// Calls that failed.
    pub failed: u64,
}

/// One session over time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionTimeline {
    /// Currency of the money fields.
    pub currency: Currency,
    /// Events in time order, at most [`TIMELINE_LIMIT`].
    pub events: Vec<TimelineEvent>,
    /// True when more events exist than returned.
    pub events_truncated: bool,
    /// Signals in time order.
    pub signals: Vec<TimelineSignal>,
    /// Tool calls per tool, most called first.
    pub tools: Vec<TimelineTool>,
}

/// Most events a [`SessionTimeline`] carries.
pub const TIMELINE_LIMIT: usize = 5000;

pub(crate) const TIMELINE_EVENTS: &str = "SELECT at_us, model, profile, provider, input_uncached, \
     cache_read, cache_write_5m, cache_write_1h, output, reasoning, \
     COALESCE(cost_api_usd_micros, 0) FROM usage_event WHERE agent = ? AND session = ? \
     AND superseded = 0 AND kind NOT IN ('daily_aggregate', 'extra_usage') \
     ORDER BY at_us, id LIMIT ?";

pub(crate) const TIMELINE_TOOLS: &str = "SELECT tool_name, mcp_server, COUNT(*), \
     COALESCE(SUM(is_error = 1), 0) FROM tool_call WHERE agent = ? AND session = ? \
     GROUP BY tool_name, mcp_server ORDER BY COUNT(*) DESC, tool_name, mcp_server";

pub(crate) const TIMELINE_SIGNALS: &str =
    "SELECT at_us, kind FROM session_signal WHERE agent = ? AND session = ? ORDER BY at_us";

/// The timeline of one session.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn session_timeline(
    reader: &StoreReader,
    ctx: &QueryContext,
    agent: AgentId,
    session: &SessionId,
) -> Result<SessionTimeline, QueryError> {
    let conn = reader.connection();
    let key = [
        Value::Text(agent.as_str().to_owned()),
        Value::Text(session.as_str().to_owned()),
    ];
    let limit = i64::try_from(TIMELINE_LIMIT).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(TIMELINE_EVENTS)?;
    let mut rows = statement.query(params_from_iter(
        key.iter().cloned().chain([Value::Integer(limit + 1)]),
    ))?;
    let mut events = Vec::new();
    let (mut api, mut paid) = (0, 0);
    while let Some(row) = rows.next()? {
        let at = micros_ts(row.get(0)?)?;
        let profile: String = row.get(2)?;
        let profile: ProfileId = profile
            .parse()
            .map_err(|_| QueryError::decode("profile", &profile))?;
        let provider: Option<String> = row.get(3)?;
        let provider: Option<ProviderId> = provider
            .map(|p| p.parse().map_err(|_| QueryError::decode("provider", &p)))
            .transpose()?;
        let count = |i: usize| -> Result<u64, QueryError> { to_count(row.get(4 + i)?) };
        let tokens = TokenCounts {
            input_uncached: count(0)?,
            cache_read: count(1)?,
            cache_write_5m: count(2)?,
            cache_write_1h: count(3)?,
            output: count(4)?,
            reasoning: count(5)?,
        };
        let local = ctx.zone().to_datetime(at).date();
        let value = usd_to(ctx.currency(), ctx.fx(), utc_date(at), row.get(10)?)?.0;
        let covered = covering(
            ctx.subscriptions(),
            agent,
            &profile,
            provider.as_ref(),
            local,
        )
        .is_some();
        let event_paid = if covered { 0 } else { value };
        api += value;
        paid += event_paid;
        events.push(TimelineEvent {
            at,
            model: row.get(1)?,
            tokens,
            api_value_micros: value,
            paid_micros: event_paid,
            cumulative_api_value_micros: api,
            cumulative_paid_micros: paid,
        });
    }
    let events_truncated = events.len() > TIMELINE_LIMIT;
    events.truncate(TIMELINE_LIMIT);
    let mut statement = conn.prepare_cached(TIMELINE_SIGNALS)?;
    let mut rows = statement.query(params_from_iter(key.iter()))?;
    let mut signals = Vec::new();
    while let Some(row) = rows.next()? {
        signals.push(TimelineSignal {
            at: micros_ts(row.get(0)?)?,
            kind: closed("signal kind", &row.get::<_, String>(1)?)?,
        });
    }
    let mut statement = conn.prepare_cached(TIMELINE_TOOLS)?;
    let mut rows = statement.query(params_from_iter(key.iter()))?;
    let mut tools = Vec::new();
    while let Some(row) = rows.next()? {
        tools.push(TimelineTool {
            tool_name: row.get(0)?,
            mcp_server: row.get(1)?,
            calls: to_count(row.get(2)?)?,
            failed: to_count(row.get(3)?)?,
        });
    }
    Ok(SessionTimeline {
        currency: ctx.currency(),
        events,
        events_truncated,
        signals,
        tools,
    })
}
