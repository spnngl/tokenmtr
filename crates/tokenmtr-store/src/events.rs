//! The `usage_event` table: one row per billed LLM call.

use std::sync::LazyLock;

use camino::Utf8Path;
use jiff::Timestamp;
use rusqlite::{Connection, Row, named_params, params};
use tokenmtr_core::{
    AgentId, DedupKey, Estimate, EventCost, EventId, EventKind, ModelId, PricingVersion, ProfileId,
    ProjectKey, ProviderId, SessionId, SourceFileId, TokenCounts, UsageEvent, UsdMicros,
};

use crate::codec::{RowExt, enum_text, from_us, to_count, to_i64, to_limit, to_us};
use crate::error::{CodecError, StoreError};
use crate::types::{Dataset, Revision, StoredEvent, UpsertStats, Versioned};
use crate::versioned::{self, Merge, UpsertSql};

/// Columns the upsert compares and overwrites. The identity (`event_id`, `agent`, `dedup_key`)
/// never changes once inserted.
const VALUES: [(&str, Merge); 25] = [
    ("profile", Merge::Replace),
    ("at_us", Merge::Replace),
    ("session", Merge::Replace),
    ("parent_session", Merge::Replace),
    ("subagent", Merge::Replace),
    ("is_sidechain", Merge::Replace),
    ("cwd", Merge::Replace),
    ("git_branch", Merge::KeepStored),
    ("project", Merge::Replace),
    ("client", Merge::Replace),
    ("source_file_id", Merge::Replace),
    ("approximate", Merge::Replace),
    ("provider", Merge::Replace),
    ("model", Merge::Replace),
    ("kind", Merge::Replace),
    ("input_uncached", Merge::Replace),
    ("cache_read", Merge::Replace),
    ("cache_write_5m", Merge::Replace),
    ("cache_write_1h", Merge::Replace),
    ("output", Merge::Replace),
    ("reasoning", Merge::Replace),
    ("cost_reported_usd_micros", Merge::Replace),
    ("cost_estimated_usd_micros", Merge::Replace),
    ("pricing_version", Merge::Replace),
    ("service_tier", Merge::Replace),
];

static UPSERT: LazyLock<UpsertSql> = LazyLock::new(|| {
    UpsertSql::new(
        "usage_event",
        &["agent", "dedup_key"],
        &["event_id", "agent", "dedup_key"],
        &VALUES,
    )
});

/// The columns [`decode`] reads.
macro_rules! select {
    ($tail:literal) => {
        concat!(
            "SELECT id, revision, event_id, agent, dedup_key, profile, at_us, session, \
             parent_session, subagent, is_sidechain, cwd, git_branch, project, client, \
             source_file_id, approximate, provider, model, kind, input_uncached, cache_read, \
             cache_write_5m, cache_write_1h, output, reasoning, cost_reported_usd_micros, \
             cost_estimated_usd_micros, pricing_version, service_tier, superseded \
             FROM usage_event ",
            $tail
        )
    };
}

const AFTER: &str = select!("WHERE revision > ?1 ORDER BY revision LIMIT ?2");
const BY_KEY: &str = select!("WHERE agent = ?1 AND dedup_key = ?2");
const BY_ID: &str = select!("WHERE event_id = ?1");
const BY_CWD: &str = select!("WHERE cwd = ?1 AND id > ?2 ORDER BY id LIMIT ?3");

/// Inserts or updates events by `(agent, dedup_key)`, last write wins.
pub(crate) fn upsert(
    conn: &Connection,
    now: Timestamp,
    events: &[UsageEvent],
) -> Result<UpsertStats, StoreError> {
    let rows = versioned::last_wins(events, |e| (e.agent, e.dedup_key.clone()));
    versioned::run(
        conn,
        Dataset::UsageEvents,
        &UPSERT,
        now,
        &rows,
        |upsert, event, revision, now_us| {
            let t = &event.tokens;
            let cost = &event.cost;
            upsert.bind(named_params! {
                        ":event_id": event.id.as_bytes(),
                        ":agent": event.agent.as_str(),
                        ":dedup_key": event.dedup_key.as_str(),
                        ":revision": revision,
                        ":updated_at_us": now_us,
                        ":profile": event.profile.as_str(),
                        ":at_us": to_us(event.at),
                        ":session": event.session.as_str(),
                        ":parent_session": event.parent_session.as_ref().map(SessionId::as_str),
                        ":subagent": event.subagent,
                        ":is_sidechain": event.is_sidechain,
                        ":cwd": event.cwd.as_ref().map(|c| c.as_str()),
                        ":git_branch": event.git_branch,
                        ":project": event.project.as_ref().map(ProjectKey::as_str),
                        ":client": event.client,
                        ":source_file_id": event.source_file.map(SourceFileId::get),
                        ":approximate": event.approximate,
                        ":provider": event.provider.as_ref().map(ProviderId::as_str),
                        ":model": event.model.as_str(),
                        ":kind": enum_text(&event.kind)?,
                        ":input_uncached": to_i64(t.input_uncached, "input_uncached")?,
                        ":cache_read": to_i64(t.cache_read, "cache_read")?,
                        ":cache_write_5m": to_i64(t.cache_write_5m, "cache_write_5m")?,
                        ":cache_write_1h": to_i64(t.cache_write_1h, "cache_write_1h")?,
                        ":output": to_i64(t.output, "output")?,
                        ":reasoning": to_i64(t.reasoning, "reasoning")?,
                        ":cost_reported_usd_micros": cost.reported().map(UsdMicros::get),
                        ":cost_estimated_usd_micros": cost.estimated().map(|e| e.usd.get()),
                        ":pricing_version": cost.estimated().map(|e| e.pricing_version.as_str()),
                        ":service_tier": event.service_tier,
            })
        },
    )
}

/// The event stored under `(agent, dedup_key)`.
pub(crate) fn by_key(
    conn: &Connection,
    agent: AgentId,
    dedup_key: &DedupKey,
) -> Result<Option<Versioned<UsageEvent>>, StoreError> {
    conn.prepare_cached(BY_KEY)?
        .query_and_then(params![agent.as_str(), dedup_key.as_str()], decode)?
        .next()
        .transpose()
}

/// The event with this id.
pub(crate) fn by_id(
    conn: &Connection,
    id: &EventId,
) -> Result<Option<Versioned<UsageEvent>>, StoreError> {
    conn.prepare_cached(BY_ID)?
        .query_and_then(params![id.as_bytes()], decode)?
        .next()
        .transpose()
}

/// Events of one working directory in insertion order, after the row `after` (exclusive).
/// Returns each event with the row id to resume from.
pub(crate) fn by_cwd(
    conn: &Connection,
    cwd: &Utf8Path,
    after: i64,
    limit: usize,
) -> Result<Vec<(i64, Versioned<UsageEvent>)>, StoreError> {
    let limit = to_limit(limit)?;
    conn.prepare_cached(BY_CWD)?
        .query_and_then(params![cwd.as_str(), after, limit], |row| {
            Ok((row.get::<_, i64>("id")?, decode(row)?))
        })?
        .collect()
}

/// When the first transcript event of `(agent, profile)` happened: not a stats-cache aggregate
/// nor an extra-usage row.
pub(crate) fn earliest_transcript(
    conn: &Connection,
    agent: AgentId,
    profile: &ProfileId,
) -> Result<Option<Timestamp>, StoreError> {
    let at: Option<i64> = conn
        .prepare_cached(
            "SELECT MIN(at_us) FROM usage_event \
             WHERE agent = ?1 AND profile = ?2 AND kind NOT IN (?3, ?4)",
        )?
        .query_row(
            params![
                agent.as_str(),
                profile.as_str(),
                enum_text(&EventKind::DailyAggregate)?,
                enum_text(&EventKind::ExtraUsage)?
            ],
            |row| row.get(0),
        )?;
    Ok(at.map(from_us).transpose()?)
}

/// Flags the stats-cache aggregates of `(agent, profile)` at or after `from` as superseded. Each
/// newly flagged row gets its own revision and the dataset counter moves once. Returns how
/// many rows changed.
pub(crate) fn supersede_aggregates_from(
    conn: &Connection,
    now: Timestamp,
    agent: AgentId,
    profile: &ProfileId,
    from: Timestamp,
) -> Result<u64, StoreError> {
    let ids = conn
        .prepare_cached(
            "SELECT id FROM usage_event WHERE agent = ?1 AND profile = ?2 AND kind = ?3 \
             AND at_us >= ?4 AND superseded = 0 ORDER BY id",
        )?
        .query_map(
            params![
                agent.as_str(),
                profile.as_str(),
                enum_text(&EventKind::DailyAggregate)?,
                to_us(from)
            ],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok(0);
    }
    let mut revision = versioned::head(conn, Dataset::UsageEvents)?;
    let mut flag = conn.prepare_cached(
        "UPDATE usage_event SET superseded = 1, revision = ?1, updated_at_us = ?2 WHERE id = ?3",
    )?;
    for id in &ids {
        revision = revision.next();
        flag.execute(params![revision, to_us(now), id])?;
    }
    conn.execute(
        "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
        params![revision, Dataset::UsageEvents.as_str()],
    )?;
    Ok(to_count(ids.len()))
}

/// Events with a revision above `after`, oldest revision first, superseded ones included.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<StoredEvent>>, StoreError> {
    let limit = to_limit(limit)?;
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, limit], decode_stored)?
        .collect()
}

fn decode_stored(row: &Row<'_>) -> Result<Versioned<StoredEvent>, StoreError> {
    let Versioned {
        revision,
        row: event,
    } = decode(row)?;
    Ok(Versioned {
        revision,
        row: StoredEvent {
            event,
            superseded: row.flag("superseded")?,
        },
    })
}

/// The estimate of a `usage_event` row: both cost columns set, or neither.
pub(crate) fn decode_estimate(row: &Row<'_>) -> Result<Option<Estimate>, StoreError> {
    let estimated_usd: Option<i64> = row.get("cost_estimated_usd_micros")?;
    let pricing_version: Option<String> = row.get("pricing_version")?;
    Ok(match (estimated_usd, pricing_version) {
        (Some(usd), Some(version)) => Some(Estimate {
            usd: UsdMicros::new(usd),
            pricing_version: PricingVersion::try_from(version).map_err(CodecError::from)?,
        }),
        (None, None) => None,
        (_, version) => {
            return Err(CodecError::Value {
                column: "pricing_version",
                value: format!("{version:?}"),
            }
            .into());
        }
    })
}

fn decode(row: &Row<'_>) -> Result<Versioned<UsageEvent>, StoreError> {
    let estimate = decode_estimate(row)?;
    let reported: Option<i64> = row.get("cost_reported_usd_micros")?;
    let cwd: Option<String> = row.get("cwd")?;
    let source_file: Option<i64> = row.get("source_file_id")?;
    Ok(Versioned {
        revision: row.get("revision")?,
        row: UsageEvent {
            id: EventId::from_bytes(row.get("event_id")?),
            agent: row.id::<AgentId>("agent")?,
            profile: row.id::<ProfileId>("profile")?,
            dedup_key: row.id::<DedupKey>("dedup_key")?,
            at: row.ts("at_us")?,
            session: row.id::<SessionId>("session")?,
            parent_session: row.id_opt::<SessionId>("parent_session")?,
            subagent: row.get("subagent")?,
            is_sidechain: row.flag("is_sidechain")?,
            cwd: cwd.map(Into::into),
            git_branch: row.get("git_branch")?,
            project: row.id_opt::<ProjectKey>("project")?,
            client: row.get("client")?,
            source_file: source_file.map(SourceFileId::new),
            approximate: row.flag("approximate")?,
            provider: row.id_opt::<ProviderId>("provider")?,
            model: row.id::<ModelId>("model")?,
            kind: row.enumeration("kind")?,
            tokens: TokenCounts {
                input_uncached: row.count("input_uncached")?,
                cache_read: row.count("cache_read")?,
                cache_write_5m: row.count("cache_write_5m")?,
                cache_write_1h: row.count("cache_write_1h")?,
                output: row.count("output")?,
                reasoning: row.count("reasoning")?,
            },
            cost: EventCost::new(reported.map(UsdMicros::new), estimate),
            service_tier: row.get("service_tier")?,
        },
    })
}
