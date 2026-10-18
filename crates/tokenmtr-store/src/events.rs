//! The `usage_event` table: one row per billed LLM call (D12).

use std::sync::LazyLock;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};
use tokenmtr_core::{
    AgentId, DedupKey, Estimate, EventCost, EventId, ModelId, PricingVersion, ProfileId,
    ProjectKey, ProviderId, SessionId, SourceFileId, TokenCounts, UsageEvent, UsdMicros,
};

use crate::codec::{RowExt, enum_text, to_i64, to_limit, to_us};
use crate::error::{CodecError, StoreError};
use crate::types::{Revision, UpsertStats, Versioned};
use crate::versioned::{self, Merge};

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
    ("git_branch", Merge::Replace),
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

static UPSERT: LazyLock<String> = LazyLock::new(|| {
    versioned::upsert_sql(
        "usage_event",
        &["agent", "dedup_key"],
        &["event_id", "agent", "dedup_key"],
        &VALUES,
    )
});

const AFTER: &str = "SELECT revision, event_id, agent, dedup_key, profile, at_us, session, \
    parent_session, subagent, is_sidechain, cwd, git_branch, project, client, source_file_id, \
    approximate, provider, model, kind, input_uncached, cache_read, cache_write_5m, \
    cache_write_1h, output, reasoning, cost_reported_usd_micros, cost_estimated_usd_micros, \
    pricing_version, service_tier \
    FROM usage_event WHERE revision > ?1 ORDER BY revision LIMIT ?2";

/// Inserts or updates events by `(agent, dedup_key)`, last write wins (D12).
pub(crate) fn upsert(
    conn: &mut Connection,
    now: Timestamp,
    events: &[UsageEvent],
) -> Result<UpsertStats, StoreError> {
    let rows = versioned::last_wins(events, |e| (e.agent, e.dedup_key.clone()));
    versioned::run(
        conn,
        crate::Dataset::UsageEvents,
        &UPSERT,
        now,
        &rows,
        |statement, event, revision, now_us| {
            let t = &event.tokens;
            let cost = &event.cost;
            Ok(statement
                .query_row(
                    named_params! {
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
                    },
                    |row| row.get::<_, i64>(0),
                )
                .optional()?)
        },
    )
}

/// Events with a revision above `after`, oldest revision first.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<UsageEvent>>, StoreError> {
    let limit = to_limit(limit)?;
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, limit], decode)?
        .collect()
}

fn decode(row: &Row<'_>) -> Result<Versioned<UsageEvent>, StoreError> {
    let estimated_usd: Option<i64> = row.get("cost_estimated_usd_micros")?;
    let pricing_version: Option<String> = row.get("pricing_version")?;
    let estimate = match (estimated_usd, pricing_version) {
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
    };
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
