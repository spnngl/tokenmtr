//! The `tool_call` table: tool name, error flag and loaded skill per invocation, never arguments.

use std::collections::HashMap;
use std::sync::LazyLock;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, named_params, params};
use tokenmtr_core::{
    AgentId, DedupKey, EventId, IdError, MachineId, ProfileId, SessionId, SkillName, ToolCall,
};

use crate::codec::{RowExt, to_limit, to_us};
use crate::error::StoreError;
use crate::types::{Dataset, Revision, UpsertStats, Versioned};
use crate::versioned::{self, Merge, UpsertSql};

/// A [`ToolCall`] with the profile of the agent that made it, which `ToolCall` lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfiledToolCall {
    /// Profile of the agent.
    pub profile: ProfileId,
    /// The call.
    pub call: ToolCall,
}

impl ProfiledToolCall {
    /// The `tool_call_id` of the export: stable per machine, agent and tool-use id, and
    /// distinct from every usage event id.
    ///
    /// # Errors
    /// Never in practice: the key is prefixed, so it is never blank.
    pub fn export_id(&self, machine: &MachineId) -> Result<EventId, IdError> {
        let key = DedupKey::try_from(format!("tool_call:{}", self.call.tool_use_id))?;
        Ok(EventId::new(machine, self.call.agent, &key))
    }
}

/// `event_id` and `is_error` are learned later (the tool result sets the flag), so a re-read of the
/// earlier line must not erase them. `skill` keeps a stored name the same way: a re-read sets a
/// missing one, never clears it.
const VALUES: [(&str, Merge); 8] = [
    ("profile", Merge::Replace),
    ("event_id", Merge::KeepStored),
    ("session", Merge::Replace),
    ("at_us", Merge::Replace),
    ("tool_name", Merge::Replace),
    ("mcp_server", Merge::Replace),
    ("is_error", Merge::KeepStored),
    ("skill", Merge::KeepStored),
];

static UPSERT: LazyLock<UpsertSql> = LazyLock::new(|| {
    UpsertSql::new(
        "tool_call",
        &["agent", "tool_use_id"],
        &["agent", "tool_use_id"],
        &VALUES,
    )
});

/// What sinks export: local rows only, imported ones stay home.
pub(crate) const AFTER: &str = "SELECT revision, agent, tool_use_id, profile, event_id, session, at_us, \
    tool_name, mcp_server, is_error, skill \
    FROM tool_call WHERE revision > ?1 AND +origin_machine IS NULL ORDER BY revision LIMIT ?2";

const BY_KEY: &str = "SELECT revision, agent, tool_use_id, profile, event_id, session, at_us, \
    tool_name, mcp_server, is_error, skill \
    FROM tool_call WHERE agent = ?1 AND tool_use_id = ?2";

/// What [`apply_result`] did to the stored call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultOutcome {
    /// The flag changed; the call got a new revision.
    Applied,
    /// The call already had this flag.
    Unchanged,
    /// No call with this id is stored.
    Unmatched,
}

/// Folds repeated tool-use ids of one batch into the row the sequential upserts would end with.
fn merge_duplicates(calls: &[ProfiledToolCall]) -> Vec<ProfiledToolCall> {
    let mut index: HashMap<(AgentId, &str), usize> = HashMap::new();
    let mut merged: Vec<ProfiledToolCall> = Vec::new();
    for incoming in calls {
        let key = (incoming.call.agent, incoming.call.tool_use_id.as_str());
        if let Some(&at) = index.get(&key) {
            let earlier = &merged[at];
            let mut next = incoming.clone();
            next.call.event = incoming.call.event.or(earlier.call.event);
            next.call.is_error = incoming.call.is_error.or(earlier.call.is_error);
            if next.call.skill.is_none() {
                next.call.skill.clone_from(&earlier.call.skill);
            }
            merged[at] = next;
        } else {
            index.insert(key, merged.len());
            merged.push(incoming.clone());
        }
    }
    merged
}

/// Inserts or updates tool calls by `(agent, tool_use_id)`.
pub(crate) fn upsert(
    conn: &Connection,
    now: Timestamp,
    calls: &[ProfiledToolCall],
) -> Result<UpsertStats, StoreError> {
    let rows = merge_duplicates(calls);
    versioned::run(
        conn,
        Dataset::ToolCalls,
        &UPSERT,
        now,
        &rows,
        |upsert, row, revision, now_us| bind(upsert, row, &row.call.tool_use_id, revision, now_us),
    )
}

/// Binds every parameter of a tool-call upsert but the origin columns, under `tool_use_id`.
pub(crate) fn bind(
    upsert: &mut versioned::Upsert<'_>,
    row: &ProfiledToolCall,
    tool_use_id: &str,
    revision: Revision,
    now_us: i64,
) -> Result<(), StoreError> {
    let call = &row.call;
    upsert.bind(named_params! {
                ":agent": call.agent.as_str(),
                ":tool_use_id": tool_use_id,
                ":revision": revision,
                ":updated_at_us": now_us,
                ":profile": row.profile.as_str(),
                ":event_id": call.event.as_ref().map(EventId::as_bytes),
                ":session": call.session.as_str(),
                ":at_us": to_us(call.at),
                ":tool_name": call.tool_name,
                ":mcp_server": call.mcp_server,
                ":is_error": call.is_error,
                ":skill": call.skill.as_ref().map(SkillName::as_str),
    })
}

/// The upsert of imported tool calls: every value column replaces (the origin is the authority),
/// plus the origin columns.
pub(crate) static UPSERT_IMPORTED: LazyLock<UpsertSql> = LazyLock::new(|| {
    let values: Vec<(&str, Merge)> = VALUES
        .iter()
        .map(|&(column, _)| (column, Merge::Replace))
        .chain(
            ["origin_machine", "origin_source", "origin_revision"]
                .map(|column| (column, Merge::Replace)),
        )
        .collect();
    UpsertSql::new(
        "tool_call",
        &["agent", "tool_use_id"],
        &["agent", "tool_use_id"],
        &values,
    )
});

/// Sets the error flag of a stored call from its tool result, bumping the revision only when the
/// flag changes.
pub(crate) fn apply_result(
    conn: &Connection,
    now: Timestamp,
    agent: AgentId,
    tool_use_id: &str,
    is_error: bool,
) -> Result<ResultOutcome, StoreError> {
    let revision = versioned::head(conn, Dataset::ToolCalls)?.next();
    let changed = conn
        .prepare_cached(
            "UPDATE tool_call SET is_error = ?1, revision = ?2, updated_at_us = ?3 \
             WHERE agent = ?4 AND tool_use_id = ?5 AND is_error IS NOT ?1",
        )?
        .execute(params![
            is_error,
            revision,
            to_us(now),
            agent.as_str(),
            tool_use_id
        ])?;
    if changed > 0 {
        conn.execute(
            "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
            params![revision, Dataset::ToolCalls.as_str()],
        )?;
        return Ok(ResultOutcome::Applied);
    }
    let stored = conn
        .prepare_cached("SELECT 1 FROM tool_call WHERE agent = ?1 AND tool_use_id = ?2")?
        .query_row(params![agent.as_str(), tool_use_id], |_| Ok(()))
        .optional()?;
    Ok(if stored.is_some() {
        ResultOutcome::Unchanged
    } else {
        ResultOutcome::Unmatched
    })
}

/// The tool call stored under `(agent, tool_use_id)`.
pub(crate) fn by_key(
    conn: &Connection,
    agent: AgentId,
    tool_use_id: &str,
) -> Result<Option<Versioned<ProfiledToolCall>>, StoreError> {
    conn.prepare_cached(BY_KEY)?
        .query_and_then(params![agent.as_str(), tool_use_id], decode)?
        .next()
        .transpose()
}

/// Tool calls with a revision above `after`, oldest revision first.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<ProfiledToolCall>>, StoreError> {
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, to_limit(limit)?], decode)?
        .collect()
}

fn decode(row: &Row<'_>) -> Result<Versioned<ProfiledToolCall>, StoreError> {
    let event: Option<[u8; 16]> = row.get("event_id")?;
    Ok(Versioned {
        revision: row.get("revision")?,
        row: ProfiledToolCall {
            profile: row.id::<ProfileId>("profile")?,
            call: ToolCall {
                agent: row.id::<AgentId>("agent")?,
                tool_use_id: row.get("tool_use_id")?,
                event: event.map(EventId::from_bytes),
                session: row.id::<SessionId>("session")?,
                at: row.ts("at_us")?,
                tool_name: row.get("tool_name")?,
                mcp_server: row.get("mcp_server")?,
                is_error: row.flag_opt("is_error")?,
                skill: row.id_opt::<SkillName>("skill")?,
            },
        },
    })
}
