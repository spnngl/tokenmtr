//! The `tool_call` table: tool name and error flag per invocation, never arguments (D32).

use std::collections::HashMap;
use std::sync::LazyLock;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};
use tokenmtr_core::{
    AgentId, DedupKey, EventId, IdError, MachineId, ProfileId, SessionId, ToolCall,
};

use crate::codec::{RowExt, to_limit, to_us};
use crate::error::StoreError;
use crate::types::{Dataset, Revision, UpsertStats, Versioned};
use crate::versioned::{self, Merge};

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
/// earlier line must not erase them.
const VALUES: [(&str, Merge); 7] = [
    ("profile", Merge::Replace),
    ("event_id", Merge::KeepStored),
    ("session", Merge::Replace),
    ("at_us", Merge::Replace),
    ("tool_name", Merge::Replace),
    ("mcp_server", Merge::Replace),
    ("is_error", Merge::KeepStored),
];

static UPSERT: LazyLock<String> = LazyLock::new(|| {
    versioned::upsert_sql(
        "tool_call",
        &["agent", "tool_use_id"],
        &["agent", "tool_use_id"],
        &VALUES,
    )
});

const AFTER: &str = "SELECT revision, agent, tool_use_id, profile, event_id, session, at_us, \
    tool_name, mcp_server, is_error \
    FROM tool_call WHERE revision > ?1 ORDER BY revision LIMIT ?2";

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
    conn: &mut Connection,
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
        |statement, row, revision, now_us| {
            let call = &row.call;
            Ok(statement
                .query_row(
                    named_params! {
                        ":agent": call.agent.as_str(),
                        ":tool_use_id": call.tool_use_id,
                        ":revision": revision,
                        ":updated_at_us": now_us,
                        ":profile": row.profile.as_str(),
                        ":event_id": call.event.as_ref().map(EventId::as_bytes),
                        ":session": call.session.as_str(),
                        ":at_us": to_us(call.at),
                        ":tool_name": call.tool_name,
                        ":mcp_server": call.mcp_server,
                        ":is_error": call.is_error,
                    },
                    |r| r.get::<_, i64>(0),
                )
                .optional()?)
        },
    )
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
            },
        },
    })
}
