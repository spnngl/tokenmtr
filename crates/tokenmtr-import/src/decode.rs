//! Export rows back into store rows: the inverse of `tokenmtr_export::project`.
//!
//! A row is a list of SQL cells in the order of `tokenmtr_export::schema`; each is read by name
//! in that order, so a column reordered in the schema fails the round-trip tests, not a file.
//! Cells the reader derives anew (`total_tokens`, `cost_source`) are not read.

use camino::Utf8PathBuf;
use jiff::Timestamp;
use rusqlite::types::Value;
use tokenmtr_core::{
    AgentId, Dataset, DedupKey, Estimate, EventCost, EventId, EventKind, MachineId, ModelId,
    PricingVersion, ProfileId, ProjectKey, ProviderId, SessionId, SkillName, TokenCounts, ToolCall,
    UsageEvent, UsdMicros,
};
use tokenmtr_export::{Column, schema};
use tokenmtr_store::{ImportedEvent, ImportedToolCall, ProfiledToolCall, Revision};

use crate::error::{DecodeError, Problem};

/// Reads the cells of one row in schema order.
struct Cells<'a> {
    columns: &'static [Column],
    values: &'a [Value],
    next: usize,
}

impl<'a> Cells<'a> {
    fn new(dataset: Dataset, values: &'a [Value]) -> Result<Self, DecodeError> {
        let columns = schema(dataset).columns;
        if columns.len() == values.len() {
            Ok(Self {
                columns,
                values,
                next: 0,
            })
        } else {
            Err(DecodeError {
                column: "*",
                problem: Problem::WrongType,
            })
        }
    }

    /// The next cell, which must be the column `name`.
    fn take(&mut self, name: &'static str) -> Result<(&'static str, &'a Value), DecodeError> {
        let cell = self
            .columns
            .get(self.next)
            .filter(|column| column.name == name)
            .zip(self.values.get(self.next));
        self.next += 1;
        cell.map(|(column, value)| (column.name, value))
            .ok_or(DecodeError {
                column: name,
                problem: Problem::WrongType,
            })
    }

    fn optional_text(&mut self, name: &'static str) -> Result<Option<&'a str>, DecodeError> {
        match self.take(name)? {
            (_, Value::Null) => Ok(None),
            (_, Value::Text(text)) => Ok(Some(text)),
            (column, _) => Err(wrong(column)),
        }
    }

    fn text(&mut self, name: &'static str) -> Result<&'a str, DecodeError> {
        self.optional_text(name)?.ok_or(DecodeError {
            column: name,
            problem: Problem::Missing,
        })
    }

    /// Text parsed into an id type; a rejected value stays out of the error.
    fn id<T: TryFrom<String>>(&mut self, name: &'static str) -> Result<T, DecodeError> {
        let text = self.text(name)?;
        T::try_from(text.to_owned()).map_err(|_| invalid(name))
    }

    fn optional_id<T: TryFrom<String>>(
        &mut self,
        name: &'static str,
    ) -> Result<Option<T>, DecodeError> {
        self.optional_text(name)?
            .map(|text| T::try_from(text.to_owned()).map_err(|_| invalid(name)))
            .transpose()
    }

    fn integer(&mut self, name: &'static str) -> Result<i64, DecodeError> {
        match self.take(name)? {
            (_, Value::Integer(n)) => Ok(*n),
            (column, Value::Null) => Err(DecodeError {
                column,
                problem: Problem::Missing,
            }),
            (column, _) => Err(wrong(column)),
        }
    }

    fn count(&mut self, name: &'static str) -> Result<u64, DecodeError> {
        u64::try_from(self.integer(name)?).map_err(|_| DecodeError {
            column: name,
            problem: Problem::OutOfRange,
        })
    }

    fn boolean(&mut self, name: &'static str) -> Result<bool, DecodeError> {
        match self.integer(name)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError {
                column: name,
                problem: Problem::OutOfRange,
            }),
        }
    }

    fn optional_boolean(&mut self, name: &'static str) -> Result<Option<bool>, DecodeError> {
        if matches!(self.values.get(self.next), Some(Value::Null)) {
            self.next += 1;
            return Ok(None);
        }
        self.boolean(name).map(Some)
    }

    fn optional_usd(&mut self, name: &'static str) -> Result<Option<UsdMicros>, DecodeError> {
        if matches!(self.values.get(self.next), Some(Value::Null)) {
            self.next += 1;
            return Ok(None);
        }
        self.integer(name)
            .map(|micros| Some(UsdMicros::new(micros)))
    }

    fn instant(&mut self, name: &'static str) -> Result<Timestamp, DecodeError> {
        self.text(name)?.parse().map_err(|_| invalid(name))
    }

    fn skip(&mut self, name: &'static str) -> Result<(), DecodeError> {
        self.take(name).map(|_| ())
    }
}

const fn wrong(column: &'static str) -> DecodeError {
    DecodeError {
        column,
        problem: Problem::WrongType,
    }
}

const fn invalid(column: &'static str) -> DecodeError {
    DecodeError {
        column,
        problem: Problem::Invalid,
    }
}

fn kind(name: &str) -> Option<EventKind> {
    Some(match name {
        "message" => EventKind::Message,
        "tool_result" => EventKind::ToolResult,
        "compaction" => EventKind::Compaction,
        "branch_summary" => EventKind::BranchSummary,
        "auxiliary" => EventKind::Auxiliary,
        "daily_aggregate" => EventKind::DailyAggregate,
        "extra_usage" => EventKind::ExtraUsage,
        _ => return None,
    })
}

/// The session of a row whose `session_id` the origin's sink dropped: one stand-in per machine,
/// because a [`SessionId`] is never blank.
fn redacted_session(machine: &MachineId) -> Result<SessionId, DecodeError> {
    SessionId::try_from(format!("redacted:{machine}")).map_err(|_| invalid("session_id"))
}

/// Reads the `machine_id` cell: it must be `machine`, the one machine a file holds.
fn row_of(machine: &MachineId, cells: &mut Cells<'_>) -> Result<(), DecodeError> {
    let found: MachineId = cells
        .text("machine_id")?
        .parse()
        .map_err(|_| invalid("machine_id"))?;
    if found == *machine {
        Ok(())
    } else {
        Err(DecodeError {
            column: "machine_id",
            problem: Problem::ForeignMachine,
        })
    }
}

/// Decodes one `usage_events` row of `machine`. `None` for a row the importer never takes:
/// `extra_usage` is account-level spend every signed-in machine polls.
///
/// # Errors
/// [`DecodeError`] naming the first cell that is not valid, or when the row belongs to another
/// machine than `machine`.
pub fn usage_event(
    machine: &MachineId,
    values: &[Value],
) -> Result<Option<ImportedEvent>, DecodeError> {
    let mut c = Cells::new(Dataset::UsageEvents, values)?;
    let id: EventId = c
        .text("event_id")?
        .parse()
        .map_err(|_| invalid("event_id"))?;
    let origin_revision = Revision::new(c.count("revision")?);
    row_of(machine, &mut c)?;
    let agent: AgentId = c.text("agent")?.parse().map_err(|_| invalid("agent"))?;
    let profile: ProfileId = c.id("profile")?;
    let provider: Option<ProviderId> = c.optional_id("provider")?;
    let model: ModelId = c.id("model")?;
    let service_tier = c.optional_text("service_tier")?.map(str::to_owned);
    let session: Option<SessionId> = c.optional_id("session_id")?;
    let parent_session: Option<SessionId> = c.optional_id("parent_session_id")?;
    let project: Option<ProjectKey> = c.optional_id("project")?;
    let client = c.optional_text("client")?.map(str::to_owned);
    let cwd = c.optional_text("cwd")?.map(Utf8PathBuf::from);
    let git_branch = c.optional_text("git_branch")?.map(str::to_owned);
    let at = c.instant("at_utc")?;
    let kind = kind(c.text("kind")?).ok_or_else(|| invalid("kind"))?;
    let approximate = c.boolean("approximate")?;
    let tokens = TokenCounts {
        input_uncached: c.count("input_uncached")?,
        cache_read: c.count("cache_read")?,
        cache_write_5m: c.count("cache_write_5m")?,
        cache_write_1h: c.count("cache_write_1h")?,
        output: c.count("output")?,
        reasoning: c.count("reasoning")?,
    };
    c.skip("total_tokens")?;
    let reported = c.optional_usd("cost_reported_usd")?;
    let estimated = c.optional_usd("cost_estimated_usd")?;
    c.skip("cost_source")?;
    let pricing_version: Option<PricingVersion> = c.optional_id("pricing_version")?;
    let superseded = c.boolean("superseded")?;
    if kind == EventKind::ExtraUsage {
        return Ok(None);
    }
    let estimate = match (estimated, pricing_version) {
        (Some(usd), Some(pricing_version)) => Some(Estimate {
            usd,
            pricing_version,
        }),
        (Some(_), None) => return Err(invalid("pricing_version")),
        (None, _) => None,
    };
    let dedup_key = DedupKey::try_from(format!("import:{id}")).map_err(|_| invalid("event_id"))?;
    let session = match session {
        Some(session) => session,
        None => redacted_session(machine)?,
    };
    Ok(Some(ImportedEvent {
        event: UsageEvent {
            id,
            agent,
            profile,
            dedup_key,
            at,
            session,
            parent_session,
            subagent: None,
            is_sidechain: false,
            cwd,
            git_branch,
            project,
            client,
            source_file: None,
            approximate,
            provider,
            model,
            kind,
            tokens,
            cost: EventCost::new(reported, estimate),
            service_tier,
        },
        superseded,
        origin_revision,
    }))
}

/// Decodes one `tool_calls` row of `machine`.
///
/// # Errors
/// [`DecodeError`] naming the first cell that is not valid, or when the row belongs to another
/// machine than `machine`.
pub fn tool_call(machine: &MachineId, values: &[Value]) -> Result<ImportedToolCall, DecodeError> {
    let mut c = Cells::new(Dataset::ToolCalls, values)?;
    let id: EventId = c
        .text("tool_call_id")?
        .parse()
        .map_err(|_| invalid("tool_call_id"))?;
    let origin_revision = Revision::new(c.count("revision")?);
    row_of(machine, &mut c)?;
    let agent: AgentId = c.text("agent")?.parse().map_err(|_| invalid("agent"))?;
    let profile: ProfileId = c.id("profile")?;
    let session: Option<SessionId> = c.optional_id("session_id")?;
    let event: Option<EventId> = c
        .optional_text("event_id")?
        .map(|text| text.parse().map_err(|_| invalid("event_id")))
        .transpose()?;
    let at = c.instant("at_utc")?;
    let tool_name = c.text("tool_name")?.to_owned();
    let mcp_server = c.optional_text("mcp_server")?.map(str::to_owned);
    let is_error = c.optional_boolean("is_error")?;
    let skill: Option<SkillName> = c.optional_id("skill")?;
    let session = match session {
        Some(session) => session,
        None => redacted_session(machine)?,
    };
    Ok(ImportedToolCall {
        id,
        call: ProfiledToolCall {
            profile,
            call: ToolCall {
                agent,
                tool_use_id: format!("import:{id}"),
                event,
                session,
                at,
                tool_name,
                mcp_server,
                is_error,
                skill,
            },
        },
        origin_revision,
    })
}
