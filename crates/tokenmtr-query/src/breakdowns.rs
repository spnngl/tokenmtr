//! Totals and breakdowns: overview, models, projects, sessions, tools, signals, heatmap.

use std::collections::{BTreeMap, HashMap};

use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use tokenmtr_core::fx::utc_date;
use tokenmtr_core::{
    AgentId, CostSource, Currency, EventKind, ModelId, PriceQuery, ProviderId, SessionId,
    SignalKind, TokenCounts, estimate, saturating_micros,
};
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{Dim, Row, Scope, closed, fetch, to_count, usd_to, where_sql};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::session_models::{SignalModel, ToolModelCalls, signal_models, tool_models};
use crate::usage::{Usage, ratio};

fn micros_ts(us: i64) -> Result<Timestamp, QueryError> {
    Timestamp::from_microsecond(us).map_err(|_| QueryError::decode("timestamp", us))
}

fn share(part: i64, whole: i64) -> f64 {
    if whole <= 0 {
        0.0
    } else {
        100.0 * ratio(part.max(0).unsigned_abs(), whole.unsigned_abs())
    }
}

/// KPI tiles of a range (Overview, recap).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Totals {
    /// Currency of every `*_micros` field except `api_value_usd_micros`.
    pub currency: Currency,
    /// True when EUR was requested but no FX rate exists, so amounts are in USD.
    pub currency_fallback: bool,
    /// Tokens, money and data quality.
    #[serde(flatten)]
    pub usage: Usage,
    /// Cached share of all input tokens, percent.
    pub cached_input_percent: f64,
    /// What cache reads saved against uncached input prices; absent without a pricing catalog.
    pub cache_savings_micros: Option<i64>,
    /// API value of subscription-covered usage divided by the fees allocated to it;
    /// absent when no subscription fee applies.
    pub roi: Option<f64>,
    /// Distinct sessions with traffic.
    pub sessions: u64,
}

/// Prices a probe event of `tokens` with `model`'s rates, `None` when the catalog lacks it.
fn probe_cost(
    ctx: &QueryContext,
    model: &ModelId,
    provider: Option<&ProviderId>,
    tokens: TokenCounts,
) -> Option<i64> {
    let query = PriceQuery {
        model,
        provider,
        tokens: &tokens,
        service_tier: None,
        modifiers: None,
    };
    estimate(&query, ctx.pricing()?, &[]).map(|p| p.estimate.usd.get())
}

/// Probe size: below the long-context price tiers (200k tokens) of every catalog entry.
const PROBE_TOKENS: u64 = 100_000;

/// Savings of `rows` (model dimension): each cached token would have cost the uncached input rate.
fn cache_savings(ctx: &QueryContext, rows: &[Row]) -> Result<Option<i64>, QueryError> {
    if ctx.pricing().is_none() {
        return Ok(None);
    }
    let mut rates: HashMap<(String, Option<ProviderId>), Option<i64>> = HashMap::new();
    let mut total = 0_i64;
    for row in rows {
        let (cell, Some(model)) = (&row.cell, row.cell.k1.as_deref()) else {
            continue;
        };
        if cell.cache_read == 0 {
            continue;
        }
        let key = (model.to_owned(), cell.provider.clone());
        let per_million = *rates.entry(key).or_insert_with(|| {
            let id = ModelId::try_from(model).ok()?;
            let uncached = probe_cost(
                ctx,
                &id,
                cell.provider.as_ref(),
                TokenCounts {
                    input_uncached: PROBE_TOKENS,
                    ..TokenCounts::default()
                },
            )?;
            let cached = probe_cost(
                ctx,
                &id,
                cell.provider.as_ref(),
                TokenCounts {
                    cache_read: PROBE_TOKENS,
                    ..TokenCounts::default()
                },
            )?;
            Some(uncached - cached)
        });
        let Some(per_million) = per_million else {
            continue;
        };
        let usd = i128::from(cell.cache_read) * i128::from(per_million) / i128::from(PROBE_TOKENS);
        let usd = saturating_micros(usd);
        total += usd_to(ctx.currency(), ctx.fx(), utc_date(cell.start), usd)?.0;
    }
    Ok(Some(total))
}

/// Totals for `filters`.
///
/// An empty store yields zeros.
///
/// # Errors
///
/// Store, SQL or decode failures; EUR fees without any rate.
pub fn overview(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Totals, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::None, Scope::All)?;
    let mut usage = Usage::default();
    for row in &fetched.rows {
        usage.add(row);
    }
    for idle in &fetched.idle {
        usage.add_idle(idle);
    }
    let savings = if ctx.pricing().is_some() {
        let by_model = fetch(reader, ctx, filters, Dim::Model, Scope::Usage)?;
        cache_savings(ctx, &by_model.rows)?
    } else {
        None
    };
    let roi = usage.roi();
    let (conditions, params) = where_sql(ctx, filters, Scope::Traffic)?;
    let sessions: i64 = reader.connection().query_row(
        &format!(
            "SELECT COUNT(*) FROM (SELECT DISTINCT agent, session FROM usage_event WHERE {conditions})"
        ),
        params_from_iter(params),
        |row| row.get(0),
    )?;
    Ok(Totals {
        currency: ctx.currency(),
        currency_fallback: ctx.currency_fallback(),
        cached_input_percent: usage.cached_input_percent(),
        usage,
        cache_savings_micros: savings,
        roi,
        sessions: to_count(sessions)?,
    })
}

/// The rows of one breakdown (models, projects, branches) and the currency of their money
/// fields, serialized as `{ currency, rows }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Breakdown<T> {
    /// Currency of every `*_micros` field of the rows except `api_value_usd_micros`, as
    /// [`Totals::currency`].
    pub currency: Currency,
    /// The rows, in the order the query documents.
    pub rows: Vec<T>,
}

/// One model of the models screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelRow {
    /// Model id as reported.
    pub model: String,
    /// Tokens, money and cost-source mix (`reported_events`, `estimated_events`, `unpriced_events`).
    #[serde(flatten)]
    pub usage: Usage,
    /// Share of the range's API value, percent.
    pub share_percent: f64,
    /// `(reported − estimated) / estimated` in percent over events that have both; absent when
    /// none do.
    pub reported_vs_estimated_percent: Option<f64>,
    /// True when that divergence exceeds 5 % either way.
    pub divergent: bool,
}

/// Models of `filters`, biggest API value first.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn models(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Breakdown<ModelRow>, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::Model, Scope::Usage)?;
    let mut by_model: BTreeMap<String, Usage> = BTreeMap::new();
    for row in &fetched.rows {
        let key = row.cell.k1.clone().unwrap_or_default();
        by_model.entry(key).or_default().add(row);
    }
    let whole: i64 = by_model.values().map(|u| u.api_value_micros).sum();
    let (conditions, params) = where_sql(ctx, filters, Scope::Usage)?;
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT model, SUM(cost_reported_usd_micros), SUM(cost_estimated_usd_micros) \
         FROM usage_event WHERE {conditions} AND cost_reported_usd_micros IS NOT NULL \
         AND cost_estimated_usd_micros IS NOT NULL GROUP BY model"
    ))?;
    let pairs: HashMap<String, (i64, i64)> = statement
        .query_map(params_from_iter(params), |row| {
            Ok((row.get(0)?, (row.get(1)?, row.get(2)?)))
        })?
        .collect::<Result<_, _>>()?;
    let mut rows: Vec<ModelRow> = by_model
        .into_iter()
        .map(|(model, usage)| {
            let divergence = pairs.get(&model).and_then(|&(reported, estimated)| {
                (estimated > 0).then(|| 100.0 * (ratio_signed(reported - estimated, estimated)))
            });
            ModelRow {
                share_percent: share(usage.api_value_micros, whole),
                divergent: divergence.is_some_and(|d| d.abs() > 5.0),
                reported_vs_estimated_percent: divergence,
                model,
                usage,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.usage
            .api_value_micros
            .cmp(&a.usage.api_value_micros)
            .then_with(|| a.model.cmp(&b.model))
    });
    Ok(Breakdown {
        currency: ctx.currency(),
        rows,
    })
}

#[expect(clippy::cast_precision_loss, reason = "display ratio of micro amounts")]
fn ratio_signed(a: i64, b: i64) -> f64 {
    a as f64 / b as f64
}

/// What the project breakdowns ([`projects`], [`crate::project_models`]) group by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectGrouping {
    /// The project key: a rule, a git remote, a repository root or the cwd.
    #[default]
    Project,
    /// The folder: the repository root of the event's cwd, else the cwd. Two clones of one
    /// remote are two folders; worktrees of one repository are one.
    Folder,
}

impl ProjectGrouping {
    pub(crate) const fn dim(self) -> Dim {
        match self {
            Self::Project => Dim::Project,
            Self::Folder => Dim::Folder,
        }
    }

    pub(crate) const fn model_dim(self) -> Dim {
        match self {
            Self::Project => Dim::ProjectModel,
            Self::Folder => Dim::FolderModel,
        }
    }
}

/// Display names of the rows of a [`ProjectGrouping`].
pub(crate) enum RowNames {
    /// From the project map, by project key.
    Projects(HashMap<String, Option<String>>),
    /// The last component of the folder path.
    Folders,
}

impl RowNames {
    pub(crate) fn load(
        reader: &StoreReader,
        grouping: ProjectGrouping,
    ) -> Result<Self, QueryError> {
        Ok(match grouping {
            ProjectGrouping::Project => Self::Projects(project_names(reader)?),
            ProjectGrouping::Folder => Self::Folders,
        })
    }

    pub(crate) fn of(&self, key: Option<&String>) -> Option<String> {
        let key = key?;
        match self {
            Self::Projects(names) => names.get(key).cloned().flatten(),
            Self::Folders => {
                let trimmed = key.trim_end_matches('/');
                let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
                Some(if last.is_empty() { key } else { last }.to_owned())
            }
        }
    }
}

/// One project (or folder, per [`ProjectGrouping`]) of the projects screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectRow {
    /// Project key, or folder path; absent for events no rule, repository or cwd could place.
    pub project: Option<String>,
    /// Display name: from the project map, or the folder's last component.
    pub name: Option<String>,
    /// Client label.
    pub client: Option<String>,
    /// Tokens and money.
    #[serde(flatten)]
    pub usage: Usage,
    /// Share of the range's API value, percent.
    pub share_percent: f64,
}

/// Projects of `filters` grouped by `grouping`, biggest paid cost first. Paid cost is each
/// project's share of the subscription fees by API value.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn projects(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    grouping: ProjectGrouping,
) -> Result<Breakdown<ProjectRow>, QueryError> {
    let fetched = fetch(reader, ctx, filters, grouping.dim(), Scope::Usage)?;
    let mut by_key: BTreeMap<(Option<String>, Option<String>), Usage> = BTreeMap::new();
    for row in &fetched.rows {
        by_key
            .entry((row.cell.k1.clone(), row.cell.k2.clone()))
            .or_default()
            .add(row);
    }
    let whole: i64 = by_key.values().map(|u| u.api_value_micros).sum();
    let names = RowNames::load(reader, grouping)?;
    let mut rows: Vec<ProjectRow> = by_key
        .into_iter()
        .map(|((project, client), usage)| ProjectRow {
            name: names.of(project.as_ref()),
            share_percent: share(usage.api_value_micros, whole),
            project,
            client,
            usage,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.usage
            .paid_micros
            .cmp(&a.usage.paid_micros)
            .then_with(|| b.usage.api_value_micros.cmp(&a.usage.api_value_micros))
            .then_with(|| a.project.cmp(&b.project))
    });
    Ok(Breakdown {
        currency: ctx.currency(),
        rows,
    })
}

/// Display names of the project map, by project key.
pub(crate) fn project_names(
    reader: &StoreReader,
) -> Result<HashMap<String, Option<String>>, QueryError> {
    let mut statement = reader.connection().prepare_cached(
        "SELECT project, MIN(name) FROM project_map WHERE project IS NOT NULL GROUP BY project",
    )?;
    let names = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(names)
}

/// One git branch of the insights screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BranchRow {
    /// Branch name; absent when the agent did not record one.
    pub branch: Option<String>,
    /// Tokens and money.
    #[serde(flatten)]
    pub usage: Usage,
}

/// Cost per git branch, biggest API value first.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn branches(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Breakdown<BranchRow>, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::Branch, Scope::Usage)?;
    let mut by_branch: BTreeMap<Option<String>, Usage> = BTreeMap::new();
    for row in &fetched.rows {
        by_branch.entry(row.cell.k1.clone()).or_default().add(row);
    }
    let mut rows: Vec<BranchRow> = by_branch
        .into_iter()
        .map(|(branch, usage)| BranchRow { branch, usage })
        .collect();
    rows.sort_by(|a, b| {
        b.usage
            .api_value_micros
            .cmp(&a.usage.api_value_micros)
            .then_with(|| a.branch.cmp(&b.branch))
    });
    Ok(Breakdown {
        currency: ctx.currency(),
        rows,
    })
}

/// A working directory no rule or repository could place into a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnresolvedCwd {
    /// The directory.
    pub cwd: String,
    /// Events in the range.
    pub events: u64,
    /// API value in USD micros.
    pub api_value_usd_micros: i64,
    /// Last event.
    pub last_at: Timestamp,
}

/// Directories resolved only to themselves, most valuable first (at most `limit`).
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn unresolved_cwds(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    limit: u32,
) -> Result<Vec<UnresolvedCwd>, QueryError> {
    let (conditions, mut params) = where_sql(ctx, filters, Scope::Traffic)?;
    params.push(Value::Integer(i64::from(limit)));
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT cwd, COUNT(*), COALESCE(SUM(cost_api_usd_micros), 0), MAX(at_us) FROM usage_event \
         WHERE {conditions} AND cwd IN (SELECT cwd FROM project_map WHERE source = 'cwd') \
         GROUP BY cwd ORDER BY 3 DESC, 1 LIMIT ?"
    ))?;
    let mut rows = statement.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let events: i64 = row.get(1)?;
        out.push(UnresolvedCwd {
            cwd: row.get(0)?,
            events: to_count(events)?,
            api_value_usd_micros: row.get(2)?,
            last_at: micros_ts(row.get(3)?)?,
        });
    }
    Ok(out)
}

/// One session row of the sessions screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionRow {
    /// Agent.
    pub agent: AgentId,
    /// Session id.
    pub session: String,
    /// Profile of the first event.
    pub profile: String,
    /// First event.
    pub started_at: Timestamp,
    /// Last event.
    pub last_at: Timestamp,
    /// Seconds between first and last event.
    pub duration_secs: i64,
    /// Most frequent project key.
    pub project: Option<String>,
    /// Models used, sorted.
    pub models: Vec<String>,
    /// Tool calls recorded for the session.
    pub tool_calls: u64,
    /// Tool calls that failed.
    pub tool_errors: u64,
    /// API errors recorded for the session.
    pub api_errors: u64,
    /// Tokens and money; `paid_micros` covers pay-as-you-go events only, fees are not split per
    /// session.
    #[serde(flatten)]
    pub usage: Usage,
}

/// A page of sessions, newest activity first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionPage {
    /// Currency of the money fields.
    pub currency: Currency,
    /// All sessions matching the filters.
    pub total: u64,
    /// Zero-based page index.
    pub page: u32,
    /// Rows per page.
    pub page_size: u32,
    /// The page.
    pub rows: Vec<SessionRow>,
}

struct SessionAcc {
    profile: String,
    first: Timestamp,
    last: Timestamp,
    usage: Usage,
}

/// Sessions (real traffic, no stats-cache daily aggregates) of `filters`.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn sessions(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    page: u32,
    page_size: u32,
) -> Result<SessionPage, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::Session, Scope::Traffic)?;
    let mut by_session: BTreeMap<(AgentId, String), SessionAcc> = BTreeMap::new();
    for row in &fetched.rows {
        let key = (row.cell.agent, row.cell.k1.clone().unwrap_or_default());
        let acc = by_session.entry(key).or_insert_with(|| SessionAcc {
            profile: row.cell.profile.as_str().to_owned(),
            first: row.cell.first_at,
            last: row.cell.last_at,
            usage: Usage::default(),
        });
        acc.first = acc.first.min(row.cell.first_at);
        acc.last = acc.last.max(row.cell.last_at);
        acc.usage.add(row);
    }
    let total = u64::try_from(by_session.len()).unwrap_or(u64::MAX);
    let mut ordered: Vec<_> = by_session.into_iter().collect();
    ordered.sort_by(|a, b| b.1.last.cmp(&a.1.last).then_with(|| a.0.cmp(&b.0)));
    let skip = usize::try_from(u64::from(page) * u64::from(page_size)).unwrap_or(usize::MAX);
    let take = usize::try_from(page_size).unwrap_or(usize::MAX);
    let paged: Vec<_> = ordered.into_iter().skip(skip).take(take).collect();
    let keys: Vec<&(AgentId, String)> = paged.iter().map(|(key, _)| key).collect();
    let mut metas = session_metas(reader, &keys)?;
    let mut rows = Vec::new();
    for ((agent, session), acc) in paged {
        let meta = metas.remove(&(agent, session.clone())).unwrap_or_default();
        rows.push(SessionRow {
            agent,
            session,
            profile: acc.profile,
            started_at: acc.first,
            last_at: acc.last,
            duration_secs: acc.last.as_second() - acc.first.as_second(),
            project: meta.project,
            models: meta.models,
            tool_calls: meta.tool_calls,
            tool_errors: meta.tool_errors,
            api_errors: meta.api_errors,
            usage: acc.usage,
        });
    }
    Ok(SessionPage {
        currency: ctx.currency(),
        total,
        page,
        page_size,
        rows,
    })
}

#[derive(Default)]
struct SessionMeta {
    project: Option<String>,
    models: Vec<String>,
    tool_calls: u64,
    tool_errors: u64,
    api_errors: u64,
}

/// Sessions per metadata query: two parameters each, far below `SQLite`'s parameter limit.
const META_CHUNK: usize = 200;

/// Project, models and error counts of `keys`, in one query per [`META_CHUNK`] sessions.
fn session_metas(
    reader: &StoreReader,
    keys: &[&(AgentId, String)],
) -> Result<HashMap<(AgentId, String), SessionMeta>, QueryError> {
    let mut out = HashMap::new();
    for chunk in keys.chunks(META_CHUNK) {
        let sql = format!(
            "WITH page(agent, session) AS (VALUES {}) SELECT p.agent, p.session, \
             (SELECT project FROM usage_event u WHERE u.agent = p.agent AND u.session = p.session \
              AND project IS NOT NULL GROUP BY project ORDER BY COUNT(*) DESC, project LIMIT 1), \
             (SELECT json_group_array(DISTINCT model) FROM usage_event u \
              WHERE u.agent = p.agent AND u.session = p.session), \
             (SELECT COUNT(*) FROM tool_call t WHERE t.agent = p.agent AND t.session = p.session), \
             (SELECT COALESCE(SUM(is_error = 1), 0) FROM tool_call t \
              WHERE t.agent = p.agent AND t.session = p.session), \
             (SELECT COUNT(*) FROM session_signal s WHERE s.agent = p.agent \
              AND s.session = p.session AND s.kind = 'api_error') \
             FROM page p",
            vec!["(?, ?)"; chunk.len()].join(", ")
        );
        let params = chunk.iter().flat_map(|(agent, session)| {
            [
                Value::Text(agent.as_str().to_owned()),
                Value::Text(session.clone()),
            ]
        });
        let mut statement = reader.connection().prepare_cached(&sql)?;
        let mut rows = statement.query(params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            let agent: String = row.get(0)?;
            let models: String = row.get(3)?;
            let mut models: Vec<String> =
                serde_json::from_str(&models).map_err(|_| QueryError::decode("models", &models))?;
            models.sort();
            out.insert(
                (
                    agent
                        .parse()
                        .map_err(|_| QueryError::decode("agent", &agent))?,
                    row.get(1)?,
                ),
                SessionMeta {
                    project: row.get(2)?,
                    models,
                    tool_calls: to_count(row.get(4)?)?,
                    tool_errors: to_count(row.get(5)?)?,
                    api_errors: to_count(row.get(6)?)?,
                },
            );
        }
    }
    Ok(out)
}

/// One usage event of a session drilldown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionEvent {
    /// When.
    pub at: Timestamp,
    /// Model.
    pub model: String,
    /// Event kind.
    pub kind: EventKind,
    /// Git branch at the time.
    pub git_branch: Option<String>,
    /// Processed tokens.
    pub total_tokens: u64,
    /// Output tokens.
    pub output: u64,
    /// API value in the result currency.
    pub api_value_micros: i64,
    /// Which cost is the API value.
    pub cost_source: CostSource,
}

/// One tool call of a session drilldown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionToolCall {
    /// When.
    pub at: Timestamp,
    /// Tool name.
    pub tool_name: String,
    /// MCP server, when it is an MCP tool.
    pub mcp_server: Option<String>,
    /// Whether it failed; absent when the agent does not say.
    pub is_error: Option<bool>,
}

/// Everything about one session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionDetail {
    /// Currency of the money fields.
    pub currency: Currency,
    /// Events in time order (at most 2000).
    pub events: Vec<SessionEvent>,
    /// True when more events exist than returned.
    pub events_truncated: bool,
    /// Tool calls in time order.
    pub tool_calls: Vec<SessionToolCall>,
    /// Signals with their time.
    pub signals: Vec<(Timestamp, SignalKind)>,
}

const DETAIL_LIMIT: i64 = 2000;

/// Drilldown of one session.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn session_detail(
    reader: &StoreReader,
    ctx: &QueryContext,
    agent: AgentId,
    session: &SessionId,
) -> Result<SessionDetail, QueryError> {
    let conn = reader.connection();
    let params = [
        Value::Text(agent.as_str().to_owned()),
        Value::Text(session.as_str().to_owned()),
    ];
    let mut statement = conn.prepare_cached(
        "SELECT at_us, model, kind, git_branch, total_tokens, output, \
         COALESCE(cost_api_usd_micros, 0), cost_source FROM usage_event \
         WHERE agent = ? AND session = ? ORDER BY at_us, id LIMIT ?",
    )?;
    let mut rows = statement.query(params_from_iter(
        params
            .iter()
            .cloned()
            .chain([Value::Integer(DETAIL_LIMIT + 1)]),
    ))?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let at = micros_ts(row.get(0)?)?;
        let usd: i64 = row.get(6)?;
        let tokens: i64 = row.get(4)?;
        let output: i64 = row.get(5)?;
        events.push(SessionEvent {
            at,
            model: row.get(1)?,
            kind: closed("event kind", &row.get::<_, String>(2)?)?,
            git_branch: row.get(3)?,
            total_tokens: to_count(tokens)?,
            output: to_count(output)?,
            api_value_micros: usd_to(ctx.currency(), ctx.fx(), utc_date(at), usd)?.0,
            cost_source: closed("cost source", &row.get::<_, String>(7)?)?,
        });
    }
    let events_truncated = i64::try_from(events.len()).unwrap_or(i64::MAX) > DETAIL_LIMIT;
    events.truncate(usize::try_from(DETAIL_LIMIT).unwrap_or(0));
    let mut statement = conn.prepare_cached(
        "SELECT at_us, tool_name, mcp_server, is_error FROM tool_call \
         WHERE agent = ? AND session = ? ORDER BY at_us, tool_use_id",
    )?;
    let mut rows = statement.query(params_from_iter(params.iter()))?;
    let mut tool_calls = Vec::new();
    while let Some(row) = rows.next()? {
        let is_error: Option<i64> = row.get(3)?;
        tool_calls.push(SessionToolCall {
            at: micros_ts(row.get(0)?)?,
            tool_name: row.get(1)?,
            mcp_server: row.get(2)?,
            is_error: is_error.map(|v| v != 0),
        });
    }
    let mut statement = conn.prepare_cached(
        "SELECT at_us, kind FROM session_signal WHERE agent = ? AND session = ? ORDER BY at_us",
    )?;
    let mut rows = statement.query(params_from_iter(params.iter()))?;
    let mut signals = Vec::new();
    while let Some(row) = rows.next()? {
        signals.push((
            micros_ts(row.get(0)?)?,
            closed("signal kind", &row.get::<_, String>(1)?)?,
        ));
    }
    Ok(SessionDetail {
        currency: ctx.currency(),
        events,
        events_truncated,
        tool_calls,
        signals,
    })
}

/// One cell of the hour × weekday heatmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HeatCell {
    /// 1 = Monday … 7 = Sunday, in the configured zone.
    pub weekday: u8,
    /// Local hour 0–23.
    pub hour: u8,
    /// Events.
    pub events: u64,
    /// Processed tokens.
    pub total_tokens: u64,
    /// API value in the result currency.
    pub api_value_micros: i64,
}

/// A full 7 × 24 grid, weekday-major.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Heatmap {
    /// Currency of the money fields.
    pub currency: Currency,
    /// 168 cells.
    pub cells: Vec<HeatCell>,
}

/// Activity by local weekday and hour (real traffic only, no stats-cache aggregates). DST hours
/// that do not exist or repeat are counted where the local clock puts them.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn heatmap(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Heatmap, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::None, Scope::Traffic)?;
    let mut cells: Vec<HeatCell> = (1..=7_u8)
        .flat_map(|weekday| {
            (0..24_u8).map(move |hour| HeatCell {
                weekday,
                hour,
                events: 0,
                total_tokens: 0,
                api_value_micros: 0,
            })
        })
        .collect();
    for row in &fetched.rows {
        let local = ctx.zone().to_datetime(row.cell.start);
        let weekday = usize::try_from(local.weekday().to_monday_zero_offset()).unwrap_or(0);
        let hour = usize::try_from(local.hour()).unwrap_or(0);
        if let Some(cell) = cells.get_mut(weekday * 24 + hour) {
            cell.events += row.cell.events;
            cell.total_tokens += row.cell.total_tokens();
            cell.api_value_micros += row.api;
        }
    }
    Ok(Heatmap {
        currency: ctx.currency(),
        cells,
    })
}

/// Calls of one tool or MCP server.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolRow {
    /// Tool name; absent in the per-server view.
    pub tool_name: Option<String>,
    /// MCP server, absent for built-in tools.
    pub mcp_server: Option<String>,
    /// Calls.
    pub calls: u64,
    /// Failed calls.
    pub errors: u64,
    /// `errors / calls`, percent.
    pub error_rate_percent: f64,
    /// Distinct sessions that used it.
    pub sessions: u64,
}

/// Tool-call insights.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolsReport {
    /// Per tool, most called first.
    pub tools: Vec<ToolRow>,
    /// Per MCP server, most called first.
    pub mcp_servers: Vec<ToolRow>,
    /// Per tool and model, most called first (see [`ToolModelCalls`] for the attribution).
    pub by_model: Vec<ToolModelCalls>,
}

fn tool_rows(
    reader: &StoreReader,
    columns: &str,
    conditions: &str,
    params: Vec<Value>,
    split: impl Fn(Option<String>, Option<String>) -> (Option<String>, Option<String>),
) -> Result<Vec<ToolRow>, QueryError> {
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT {columns}, COUNT(*), COALESCE(SUM(is_error = 1), 0), \
         COUNT(DISTINCT agent || char(0) || session) FROM tool_call WHERE {conditions} \
         GROUP BY {columns} ORDER BY 3 DESC, 1, 2"
    ))?;
    let mut rows = statement.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let (tool_name, mcp_server) = split(row.get(0)?, row.get(1)?);
        let (calls, errors, sessions): (i64, i64, i64) = (row.get(2)?, row.get(3)?, row.get(4)?);
        let calls = to_count(calls)?;
        let errors = to_count(errors)?;
        out.push(ToolRow {
            tool_name,
            mcp_server,
            calls,
            errors,
            error_rate_percent: if calls == 0 {
                0.0
            } else {
                100.0 * ratio(errors, calls)
            },
            sessions: to_count(sessions)?,
        });
    }
    Ok(out)
}

/// Tool calls in the range. Only the range, agents and profiles of `filters` apply: tool calls
/// carry no model, project or client. `by_model` attributes each call to the model its session
/// used last before it.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn tools(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<ToolsReport, QueryError> {
    let (from, to) = filters.range().instants(ctx.zone())?;
    let mut conditions = String::from("at_us >= ? AND at_us < ?");
    let mut params = vec![
        Value::Integer(from.as_microsecond()),
        Value::Integer(to.as_microsecond()),
    ];
    filters.sql_agent_profile(&mut conditions, &mut params);
    let tools = tool_rows(
        reader,
        "tool_name, mcp_server",
        &conditions,
        params.clone(),
        |a, b| (a, b),
    )?;
    let servers = tool_rows(
        reader,
        "mcp_server, NULL",
        &format!("{conditions} AND mcp_server IS NOT NULL"),
        params.clone(),
        |server, _| (None, server),
    )?;
    Ok(ToolsReport {
        tools,
        mcp_servers: servers,
        by_model: tool_models(reader, &conditions, params)?,
    })
}

/// API errors and compactions of one local day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SignalDay {
    /// Local day.
    pub date: Date,
    /// API errors.
    pub api_errors: u64,
    /// Compactions.
    pub compactions: u64,
}

/// Compactions of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionCompactions {
    /// Agent.
    pub agent: AgentId,
    /// Session id.
    pub session: String,
    /// Compactions in the range.
    pub compactions: u64,
}

/// Error and compaction signals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalsReport {
    /// Every local day of the range, zero-filled.
    pub days: Vec<SignalDay>,
    /// Total API errors.
    pub api_errors: u64,
    /// Total compactions.
    pub compactions: u64,
    /// The ten sessions with most compactions.
    pub top_sessions: Vec<SessionCompactions>,
    /// Per model, most signals first (see [`SignalModel`] for the attribution).
    pub models: Vec<SignalModel>,
}

/// Signals in the range; only range and agents of `filters` apply. `models` attributes each
/// signal to the model its session used last before it.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn signals(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<SignalsReport, QueryError> {
    let (from, to) = filters.range().instants(ctx.zone())?;
    let mut conditions = String::from("at_us >= ? AND at_us < ?");
    let mut params = vec![
        Value::Integer(from.as_microsecond()),
        Value::Integer(to.as_microsecond()),
    ];
    filters.sql_agents(&mut conditions, &mut params);
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT at_us - ((at_us % 900000000) + 900000000) % 900000000, kind, COUNT(*) \
         FROM session_signal WHERE {conditions} GROUP BY 1, 2"
    ))?;
    let mut days: BTreeMap<Date, SignalDay> = filters
        .range()
        .dates()
        .map(|date| {
            (
                date,
                SignalDay {
                    date,
                    api_errors: 0,
                    compactions: 0,
                },
            )
        })
        .collect();
    let (mut api_errors, mut compactions) = (0, 0);
    let mut rows = statement.query(params_from_iter(params.iter().cloned()))?;
    while let Some(row) = rows.next()? {
        let date = ctx.zone().to_datetime(micros_ts(row.get(0)?)?).date();
        let kind: String = row.get(1)?;
        let n: i64 = row.get(2)?;
        let n = to_count(n)?;
        if let Some(day) = days.get_mut(&date) {
            match kind.as_str() {
                "api_error" => day.api_errors += n,
                "compaction" => day.compactions += n,
                _ => {}
            }
        }
        match kind.as_str() {
            "api_error" => api_errors += n,
            "compaction" => compactions += n,
            _ => {}
        }
    }
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT agent, session, COUNT(*) FROM session_signal WHERE {conditions} \
         AND kind = 'compaction' GROUP BY agent, session ORDER BY 3 DESC, 1, 2 LIMIT 10"
    ))?;
    let mut rows = statement.query(params_from_iter(params.iter().cloned()))?;
    let mut top_sessions = Vec::new();
    while let Some(row) = rows.next()? {
        let agent: String = row.get(0)?;
        let n: i64 = row.get(2)?;
        top_sessions.push(SessionCompactions {
            agent: agent
                .parse()
                .map_err(|_| QueryError::decode("agent", &agent))?,
            session: row.get(1)?,
            compactions: to_count(n)?,
        });
    }
    Ok(SignalsReport {
        days: days.into_values().collect(),
        api_errors,
        compactions,
        top_sessions,
        models: signal_models(reader, &conditions, params)?,
    })
}
