//! SQL over a [`StoreReader`], once per refresh, into a [`Snapshot`].
//!
//! The snapshot is fine-grained on purpose: label tuple, `cwd` and project key. Each output
//! then folds it with its own exclusions and redaction (`scope.rs`) without another SQL pass.
//! Every query runs in one deferred read transaction, so under WAL they all see one commit.
//!
//! `billing` follows the query engine's rule through [`tokenmtr_core::covering`]. Coverage
//! changes only at a subscription's start and end, so SQL groups rows by the *segment* between
//! those instants and the rule runs once per `(agent, profile, provider, segment)`.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::str::FromStr;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};
use tokenmtr_core::{AgentId, ProfileId, ProviderId, Subscription, covering};
use tokenmtr_store::StoreReader;

use crate::catalog::TOKEN_TYPES;
use crate::error::MetricsError;

/// Rows that count: not a superseded row, never a daily aggregate (a re-import would move it) or
/// an extra-usage row (money, no tokens).
pub(crate) const COUNTED: &str =
    "superseded = 0 AND kind NOT IN ('daily_aggregate', 'extra_usage')";

/// What a snapshot needs besides the store.
#[derive(Debug, Clone)]
pub struct SnapshotContext {
    zone: TimeZone,
    subscriptions: Vec<Subscription>,
    session_window: SignedDuration,
}

impl SnapshotContext {
    /// `zone` is `display.timezone`: the calendar the `billing` label follows.
    /// `session_window` is `[metrics].session_window`.
    #[must_use]
    pub const fn new(
        zone: TimeZone,
        subscriptions: Vec<Subscription>,
        session_window: SignedDuration,
    ) -> Self {
        Self {
            zone,
            subscriptions,
            session_window,
        }
    }
}

/// Where a group of rows came from: what exclusions match. All `None` is never excluded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) struct Origin {
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub name: Option<String>,
    pub remote: Option<String>,
}

/// Usage rows of one label tuple, origin and (for session families) session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageGroup {
    pub origin: usize,
    pub agent: String,
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub client: String,
    pub project: String,
    pub account: String,
    pub billing: &'static str,
    pub kind: String,
    /// `session_id` and `git_branch`, in session families only.
    pub session: Option<(String, String)>,
    /// In the order of [`TOKEN_TYPES`].
    pub tokens: [u64; TOKEN_TYPES.len()],
    pub reasoning: u64,
    pub events: u64,
    pub created_us: i64,
}

/// Tool calls of one tool, origin and (for session families) session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolGroup {
    pub origin: usize,
    pub agent: String,
    pub profile: String,
    pub tool: String,
    pub mcp_server: String,
    pub error: &'static str,
    pub session: Option<String>,
    pub calls: u64,
    pub created_us: i64,
}

/// The newest successful poll of one quota window.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaGauge {
    pub agent: String,
    pub profile: String,
    pub account: String,
    pub window: String,
    pub used_ratio: Option<f64>,
    pub resets_at_us: Option<i64>,
    pub fetched_at_us: i64,
}

/// The store as the metrics see it at one instant. Fold it per output.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub(crate) taken_at: Timestamp,
    pub(crate) origins: Vec<Origin>,
    pub(crate) usage: Vec<UsageGroup>,
    pub(crate) session_usage: Vec<UsageGroup>,
    pub(crate) tools: Vec<ToolGroup>,
    pub(crate) session_tools: Vec<ToolGroup>,
    pub(crate) quota: Vec<QuotaGauge>,
    next_expiry: Option<Timestamp>,
}

impl Snapshot {
    /// When the snapshot was computed.
    #[must_use]
    pub const fn taken_at(&self) -> Timestamp {
        self.taken_at
    }

    /// When the first active session leaves the session window: the instant a refresh is due
    /// even if nothing is written (`earliest last event + session_window`). `None` without an
    /// active session.
    #[must_use]
    pub const fn next_expiry(&self) -> Option<Timestamp> {
        self.next_expiry
    }
}

/// Reads the store into a [`Snapshot`], inside one read transaction.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn snapshot(
    reader: &StoreReader,
    ctx: &SnapshotContext,
    now: Timestamp,
) -> Result<Snapshot, MetricsError> {
    let transaction = reader.connection().unchecked_transaction()?;
    read(&transaction, ctx, now)
}

fn read(db: &Connection, ctx: &SnapshotContext, now: Timestamp) -> Result<Snapshot, MetricsError> {
    let segments = Segments::new(&ctx.zone, &ctx.subscriptions)?;
    let accounts = accounts(db)?;
    let since = now.checked_sub(ctx.session_window)?.as_microsecond();
    let mut origins = Origins::default();
    let mut billing = Billing::new(&ctx.subscriptions, &segments);

    let usage = usage_rows(db, &segments, &accounts, &mut billing, &mut origins, None)?
        .into_iter()
        .map(|(group, _)| group)
        .collect();
    let (session_usage, active, next_expiry) = {
        let groups = usage_rows(
            db,
            &segments,
            &accounts,
            &mut billing,
            &mut origins,
            Some(since),
        )?;
        let mut last: HashMap<(&str, &str), i64> = HashMap::new();
        for (group, last_at) in &groups {
            if let Some((id, _)) = &group.session {
                let slot = last
                    .entry((group.agent.as_str(), id.as_str()))
                    .or_insert(i64::MIN);
                *slot = (*slot).max(*last_at);
            }
        }
        let live: HashSet<(String, String)> = last
            .iter()
            .filter(|(_, at)| **at >= since)
            .map(|((agent, id), _)| ((*agent).to_owned(), (*id).to_owned()))
            .collect();
        let earliest = last.values().copied().filter(|at| *at >= since).min();
        let expiry = earliest
            .map(Timestamp::from_microsecond)
            .transpose()?
            .map(|at| at.checked_add(ctx.session_window))
            .transpose()?;
        let kept = groups
            .into_iter()
            .map(|(group, _)| group)
            .filter(|g| {
                g.session
                    .as_ref()
                    .is_some_and(|(id, _)| live.contains(&(g.agent.clone(), id.clone())))
            })
            .collect();
        (kept, live, expiry)
    };

    let mut resolver = Fallback::default();
    let totals = tools(db, &mut origins, &mut resolver, None)?;
    let mut session_tools = tools(db, &mut origins, &mut resolver, Some(since))?;
    session_tools.retain(|t| {
        t.session
            .as_ref()
            .is_some_and(|id| active.contains(&(t.agent.clone(), id.clone())))
    });
    let quota = quota(db, &accounts)?;

    Ok(Snapshot {
        taken_at: now,
        origins: origins.list,
        usage,
        session_usage,
        tools: totals,
        session_tools,
        quota,
        next_expiry,
    })
}

/// Interned [`Origin`]s: a store has few distinct working directories, many groups.
#[derive(Default)]
struct Origins {
    list: Vec<Origin>,
    index: HashMap<Origin, usize>,
}

impl Origins {
    fn intern(&mut self, origin: Origin) -> usize {
        if let Some(&at) = self.index.get(&origin) {
            return at;
        }
        let at = self.list.len();
        self.index.insert(origin.clone(), at);
        self.list.push(origin);
        at
    }
}

/// The instants where billing coverage can change, as UTC microseconds of local midnight.
pub(crate) struct Segments {
    /// Sorted, deduplicated first days of coverage and first days after it.
    dates: Vec<Date>,
    bounds_us: Vec<i64>,
}

impl Segments {
    pub(crate) fn new(
        zone: &TimeZone,
        subscriptions: &[Subscription],
    ) -> Result<Self, MetricsError> {
        let mut dates: Vec<Date> = subscriptions
            .iter()
            .flat_map(|s| std::iter::once(s.start()).chain(s.end()))
            .collect();
        dates.sort_unstable();
        dates.dedup();
        let bounds_us = dates
            .iter()
            .map(|d| Ok(d.to_zoned(zone.clone())?.timestamp().as_microsecond()))
            .collect::<Result<_, jiff::Error>>()?;
        Ok(Self { dates, bounds_us })
    }

    /// SQL for the index of the segment `column` falls in: how many bounds are at or before it.
    /// The bounds are bound as the first parameters; their count is generated, never a value.
    pub(crate) fn expression(&self, column: &str) -> String {
        if self.bounds_us.is_empty() {
            return "CAST(0 AS INTEGER)".to_owned();
        }
        (1..=self.bounds_us.len())
            .map(|n| format!("({column} >= ?{n})"))
            .collect::<Vec<_>>()
            .join(" + ")
    }

    pub(crate) fn params(&self) -> Vec<Value> {
        self.bounds_us.iter().copied().map(Value::Integer).collect()
    }

    /// A local day inside segment `index`; `None` for the segment before every subscription.
    fn day(&self, index: usize) -> Option<Date> {
        index
            .checked_sub(1)
            .and_then(|i| self.dates.get(i))
            .copied()
    }
}

/// `api` or `subscription` per `(agent, profile, provider, segment)`, by the shared rule.
struct Billing<'a> {
    subscriptions: &'a [Subscription],
    segments: &'a Segments,
    cache: HashMap<(String, String, String, usize), &'static str>,
}

impl<'a> Billing<'a> {
    fn new(subscriptions: &'a [Subscription], segments: &'a Segments) -> Self {
        Self {
            subscriptions,
            segments,
            cache: HashMap::new(),
        }
    }

    fn label(
        &mut self,
        agent: &str,
        profile: &str,
        provider: &str,
        segment: usize,
    ) -> Result<&'static str, MetricsError> {
        let key = (
            agent.to_owned(),
            profile.to_owned(),
            provider.to_owned(),
            segment,
        );
        if let Some(label) = self.cache.get(&key) {
            return Ok(*label);
        }
        let label = match self.segments.day(segment) {
            None => "api",
            Some(day) => {
                let agent_id =
                    AgentId::from_str(agent).map_err(|_| MetricsError::decode("agent", agent))?;
                let profile_id = ProfileId::try_from(profile.to_owned())
                    .map_err(|_| MetricsError::decode("profile", profile))?;
                let provider_id = (!provider.is_empty())
                    .then(|| ProviderId::try_from(provider.to_owned()))
                    .transpose()
                    .map_err(|_| MetricsError::decode("provider", provider))?;
                if covering(
                    self.subscriptions,
                    agent_id,
                    &profile_id,
                    provider_id.as_ref(),
                    day,
                )
                .is_some()
                {
                    "subscription"
                } else {
                    "api"
                }
            }
        };
        self.cache.insert(key, label);
        Ok(label)
    }
}

/// `(agent, profile)` to the account label: email, else organization, else empty.
fn accounts(db: &Connection) -> Result<HashMap<(String, String), String>, MetricsError> {
    let mut statement = db.prepare_cached(
        "SELECT agent, profile, COALESCE(NULLIF(email, ''), NULLIF(organization, ''), '') \
         FROM account",
    )?;
    let rows = statement.query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The usage rows, one per event. `sessions` selects the session and its branch and keeps only
/// sessions with a counted event since `?{n + 1}`, the active-session prefilter.
///
/// Rows are grouped in Rust, not by `GROUP BY`: without an index in grouping order SQLite sorts
/// every row, 4× slower than hashing them on a million-event store (see `benches/metrics.rs`).
pub(crate) fn usage_sql(segments: &Segments, sessions: bool) -> String {
    let segment = segments.expression("u.at_us");
    let (columns, filter) = if sessions {
        (
            "u.session, u.git_branch",
            format!(
                " AND u.session IN (SELECT session FROM usage_event \
                 WHERE at_us >= ?{} AND {COUNTED})",
                segments.bounds_us.len() + 1
            ),
        )
    } else {
        ("NULL, NULL", String::new())
    };
    format!(
        "SELECT u.agent, u.profile, u.provider, u.model, u.client, u.kind, u.cwd, u.project, \
         {columns}, {segment}, u.at_us, u.input_uncached, u.cache_read, u.cache_write_5m, \
         u.cache_write_1h, u.output, u.reasoning \
         FROM usage_event u \
         WHERE u.superseded = 0 AND u.kind NOT IN ('daily_aggregate', 'extra_usage'){filter}"
    )
}

/// Columns of a usage row that make a group: agent, profile, provider, model, client, kind, cwd,
/// project, session, git branch.
const USAGE_DIMENSIONS: usize = 10;

/// The numbers of one row, after its `N` text dimensions.
#[derive(Clone, Copy)]
struct Measures {
    segment: usize,
    at_us: i64,
    tokens: [u64; TOKEN_TYPES.len()],
    reasoning: u64,
}

/// What one group sums up.
struct Sums<const N: usize> {
    dimensions: [Option<String>; N],
    segment: usize,
    tokens: [u64; TOKEN_TYPES.len()],
    reasoning: u64,
    events: u64,
    first_us: i64,
    last_us: i64,
}

/// Rows hashed into groups without allocating per row: a group is looked up by the hash of its
/// borrowed dimensions, and only a new group copies them.
struct Grouping<const N: usize> {
    hasher: RandomState,
    index: HashMap<u64, Vec<usize>>,
    groups: Vec<Sums<N>>,
}

impl<const N: usize> Default for Grouping<N> {
    fn default() -> Self {
        Self {
            hasher: RandomState::new(),
            index: HashMap::new(),
            groups: Vec::new(),
        }
    }
}

impl<const N: usize> Grouping<N> {
    /// Adds a row whose first `N` columns are the dimensions.
    fn add(&mut self, row: &Row<'_>, measures: &Measures) -> Result<(), MetricsError> {
        let Measures {
            segment,
            at_us,
            tokens,
            reasoning,
        } = *measures;
        let mut dimensions: [Option<&str>; N] = [None; N];
        for (at, slot) in dimensions.iter_mut().enumerate() {
            *slot = row
                .get_ref(at)?
                .as_str_or_null()
                .map_err(|e| MetricsError::decode("text column", e.to_string()))?;
        }
        let mut hasher = self.hasher.build_hasher();
        dimensions.hash(&mut hasher);
        segment.hash(&mut hasher);
        let candidates = self.index.entry(hasher.finish()).or_default();
        let found = candidates.iter().copied().find(|&at| {
            let group = &self.groups[at];
            group.segment == segment
                && group
                    .dimensions
                    .iter()
                    .zip(&dimensions)
                    .all(|(own, row)| own.as_deref() == *row)
        });
        let at = if let Some(at) = found {
            at
        } else {
            candidates.push(self.groups.len());
            self.groups.push(Sums {
                dimensions: dimensions.map(|d| d.map(str::to_owned)),
                segment,
                tokens: [0; TOKEN_TYPES.len()],
                reasoning: 0,
                events: 0,
                first_us: at_us,
                last_us: at_us,
            });
            self.groups.len() - 1
        };
        let group = &mut self.groups[at];
        for (slot, add) in group.tokens.iter_mut().zip(tokens) {
            *slot = slot.saturating_add(add);
        }
        group.reasoning = group.reasoning.saturating_add(reasoning);
        group.events += 1;
        group.first_us = group.first_us.min(at_us);
        group.last_us = group.last_us.max(at_us);
        Ok(())
    }
}

/// `cwd` to `(name, remote)`.
fn directory(db: &Connection) -> Result<HashMap<String, (String, Option<String>)>, MetricsError> {
    let mut statement = db.prepare_cached("SELECT cwd, name, remote FROM project_map")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Groups with the instant of their newest row.
fn usage_rows(
    db: &Connection,
    segments: &Segments,
    accounts: &HashMap<(String, String), String>,
    billing: &mut Billing<'_>,
    origins: &mut Origins,
    since: Option<i64>,
) -> Result<Vec<(UsageGroup, i64)>, MetricsError> {
    let mut params = segments.params();
    params.extend(since.map(Value::Integer));
    let mut grouping = Grouping::<USAGE_DIMENSIONS>::default();
    {
        let mut statement = db.prepare_cached(&usage_sql(segments, since.is_some()))?;
        let mut rows = statement.query(params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            let mut tokens = [0; TOKEN_TYPES.len()];
            for (slot, column) in tokens.iter_mut().zip(12..) {
                *slot = count(row.get(column)?, "tokens")?;
            }
            grouping.add(
                row,
                &Measures {
                    segment: count(row.get(10)?, "segment")?,
                    at_us: row.get(11)?,
                    tokens,
                    reasoning: count(row.get(17)?, "reasoning")?,
                },
            )?;
        }
    }
    let projects = directory(db)?;
    grouping
        .groups
        .into_iter()
        .map(|sums| usage_group(sums, &projects, accounts, billing, origins))
        .collect()
}

fn usage_group(
    sums: Sums<USAGE_DIMENSIONS>,
    projects: &HashMap<String, (String, Option<String>)>,
    accounts: &HashMap<(String, String), String>,
    billing: &mut Billing<'_>,
    origins: &mut Origins,
) -> Result<(UsageGroup, i64), MetricsError> {
    let Sums {
        dimensions:
            [
                agent,
                profile,
                provider,
                model,
                client,
                kind,
                cwd,
                project,
                session,
                branch,
            ],
        segment,
        tokens,
        reasoning,
        events,
        first_us,
        last_us,
    } = sums;
    let required =
        |value: Option<String>, what| value.ok_or_else(|| MetricsError::decode(what, "NULL"));
    let agent = required(agent, "agent")?;
    let profile = required(profile, "profile")?;
    let provider = provider.unwrap_or_default();
    let origin = origin_of(cwd, project, projects);
    let account = accounts
        .get(&(agent.clone(), profile.clone()))
        .cloned()
        .unwrap_or_default();
    let group = UsageGroup {
        billing: billing.label(&agent, &profile, &provider, segment)?,
        project: project_label(&origin),
        origin: origins.intern(origin),
        account,
        model: required(model, "model")?,
        client: client.unwrap_or_default(),
        kind: required(kind, "kind")?,
        session: session.map(|id| (id, branch.unwrap_or_default())),
        tokens,
        reasoning,
        events,
        created_us: first_us,
        agent,
        profile,
        provider,
    };
    Ok((group, last_us))
}

/// The `project` label: the complete git remote (`host/owner/repo`), else the directory name,
/// else the last segment of the project key. Never the key itself: it can be a path.
fn project_label(origin: &Origin) -> String {
    origin
        .remote
        .clone()
        .or_else(|| origin.name.clone())
        .or_else(|| {
            origin
                .project
                .as_deref()
                .and_then(|key| key.trim_end_matches('/').rsplit('/').next())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Columns of a tool-call row that make a group: agent, profile, tool, MCP server, error flag,
/// event cwd, event project, the session of a call without a stored event, session.
const TOOL_DIMENSIONS: usize = 9;

/// Tool calls. Totals take the origin from the call's usage event; a call whose event is not
/// stored yet carries its session in column 7, to be resolved by [`Fallback`]. Grouped in Rust,
/// like [`usage_sql`].
pub(crate) fn tools_sql(sessions: bool) -> String {
    let (column, filter) = if sessions {
        (
            "t.session",
            format!(
                " WHERE t.session IN (SELECT session FROM usage_event \
                 WHERE at_us >= ?1 AND {COUNTED})"
            ),
        )
    } else {
        ("NULL", String::new())
    };
    format!(
        "SELECT t.agent, t.profile, t.tool_name, COALESCE(t.mcp_server, ''), \
         CASE WHEN t.is_error IS NULL THEN 'unknown' WHEN t.is_error = 0 THEN 'false' \
              ELSE 'true' END, \
         e.cwd, e.project, CASE WHEN e.cwd IS NULL AND e.project IS NULL THEN t.session END, {column}, t.at_us \
         FROM tool_call t LEFT JOIN usage_event e ON e.event_id = t.event_id{filter}"
    )
}

fn tools(
    db: &Connection,
    origins: &mut Origins,
    fallback: &mut Fallback,
    since: Option<i64>,
) -> Result<Vec<ToolGroup>, MetricsError> {
    let mut grouping = Grouping::<TOOL_DIMENSIONS>::default();
    {
        let mut statement = db.prepare_cached(&tools_sql(since.is_some()))?;
        let mut rows = statement.query(params_from_iter(since.map(Value::Integer)))?;
        while let Some(row) = rows.next()? {
            grouping.add(
                row,
                &Measures {
                    segment: 0,
                    at_us: row.get(TOOL_DIMENSIONS)?,
                    tokens: [0; TOKEN_TYPES.len()],
                    reasoning: 0,
                },
            )?;
        }
    }
    let projects = directory(db)?;
    grouping
        .groups
        .into_iter()
        .map(|sums| tool_group(db, sums, &projects, origins, fallback))
        .collect()
}

fn tool_group(
    db: &Connection,
    sums: Sums<TOOL_DIMENSIONS>,
    projects: &HashMap<String, (String, Option<String>)>,
    origins: &mut Origins,
    fallback: &mut Fallback,
) -> Result<ToolGroup, MetricsError> {
    let Sums {
        dimensions:
            [
                agent,
                profile,
                tool,
                mcp_server,
                error,
                cwd,
                project,
                unresolved,
                session,
            ],
        events,
        first_us,
        ..
    } = sums;
    let agent = agent.ok_or_else(|| MetricsError::decode("agent", "NULL"))?;
    let origin = match unresolved {
        Some(unresolved) => fallback.resolve(db, &agent, &unresolved)?,
        None => origin_of(cwd, project, projects),
    };
    Ok(ToolGroup {
        origin: origins.intern(origin),
        profile: profile.unwrap_or_default(),
        tool: tool.unwrap_or_default(),
        mcp_server: mcp_server.unwrap_or_default(),
        error: match error.as_deref() {
            Some("false") => "false",
            Some("true") => "true",
            _ => "unknown",
        },
        session,
        calls: events,
        created_us: first_us,
        agent,
    })
}

/// The origin of a `cwd` and project key, with the directory's name and remote.
fn origin_of(
    cwd: Option<String>,
    project: Option<String>,
    projects: &HashMap<String, (String, Option<String>)>,
) -> Origin {
    let (name, remote) = cwd
        .as_ref()
        .and_then(|cwd| projects.get(cwd))
        .map_or((None, None), |(name, remote)| {
            (Some(name.clone()), remote.clone())
        });
    Origin {
        cwd,
        project,
        name,
        remote,
    }
}

/// The origin lookup of [`Fallback`]; rides `usage_event_session (session, at_us)`.
pub(crate) const FALLBACK_SQL: &str = "SELECT u.cwd, u.project, m.name, m.remote \
     FROM usage_event u LEFT JOIN project_map m ON m.cwd = u.cwd \
     WHERE u.session = ?1 AND u.agent = ?2 AND u.superseded = 0 \
       AND u.kind NOT IN ('daily_aggregate', 'extra_usage') \
       AND (u.cwd IS NOT NULL OR u.project IS NOT NULL) \
     ORDER BY u.at_us LIMIT 1";

/// The origin of a tool call whose event is unknown: its session's earliest counted event that
/// has a cwd or a project. A tool call never inherits "nothing" when a later event knows better.
#[derive(Default)]
struct Fallback {
    cache: HashMap<(String, String), Origin>,
}

impl Fallback {
    fn resolve(
        &mut self,
        db: &Connection,
        agent: &str,
        session: &str,
    ) -> Result<Origin, MetricsError> {
        let key = (agent.to_owned(), session.to_owned());
        if let Some(origin) = self.cache.get(&key) {
            return Ok(origin.clone());
        }
        let mut statement = db.prepare_cached(FALLBACK_SQL)?;
        let origin = statement
            .query_map([session, agent], |row| {
                Ok(Origin {
                    cwd: row.get(0)?,
                    project: row.get(1)?,
                    name: row.get(2)?,
                    remote: row.get(3)?,
                })
            })?
            .next()
            .transpose()?
            .unwrap_or_default();
        self.cache.insert(key, origin.clone());
        Ok(origin)
    }
}

/// The newest successful poll of every profile, one gauge row per window. Money windows
/// (`extra_usage`) are left out: they carry spend, not utilization.
pub(crate) const QUOTA_SQL: &str = "SELECT q.agent, q.profile, q.window_id, \
     COALESCE(NULLIF(q.account, ''), ''), q.used_percent, q.resets_at_us, q.fetched_at_us \
     FROM (SELECT DISTINCT agent, profile FROM quota_snapshot) p \
     JOIN quota_snapshot q ON q.agent = p.agent AND q.profile = p.profile \
       AND q.status = 'ok' AND q.window_id <> 'extra_usage' \
       AND q.fetched_at_us = (SELECT MAX(fetched_at_us) FROM quota_snapshot \
                              WHERE agent = p.agent AND profile = p.profile AND status = 'ok') \
     ORDER BY q.agent, q.profile, q.window_id";

fn quota(
    db: &Connection,
    accounts: &HashMap<(String, String), String>,
) -> Result<Vec<QuotaGauge>, MetricsError> {
    let mut statement = db.prepare_cached(QUOTA_SQL)?;
    let mut rows = statement.query([])?;
    let mut gauges = Vec::new();
    while let Some(row) = rows.next()? {
        let agent: String = row.get(0)?;
        let profile: String = row.get(1)?;
        let polled: String = row.get(3)?;
        let account = accounts
            .get(&(agent.clone(), profile.clone()))
            .filter(|a| !a.is_empty())
            .cloned()
            .unwrap_or(polled);
        gauges.push(QuotaGauge {
            account,
            window: row.get(2)?,
            used_ratio: row.get::<_, Option<f64>>(4)?.map(|p| p / 100.0),
            resets_at_us: row.get(5)?,
            fetched_at_us: row.get(6)?,
            agent,
            profile,
        });
    }
    Ok(gauges)
}

/// A SQL count or sum as an unsigned number; a negative value means a corrupt store.
fn count<T: TryFrom<i64>>(value: i64, what: &'static str) -> Result<T, MetricsError> {
    T::try_from(value).map_err(|_| MetricsError::decode(what, value.to_string()))
}
