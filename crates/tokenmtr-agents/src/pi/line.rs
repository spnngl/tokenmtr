//! pi line parser: borrowed, partial serde structs that read usage and tool metadata, never
//! content, as for Claude Code. Fields we do not read are skipped without allocating.
//!
//! Usage hides in five places, and only the assistant one names its model:
//!
//! | Entry                                   | Event kind      | Provider and model                   |
//! |-----------------------------------------|-----------------|--------------------------------------|
//! | `message`, assistant, `message.usage`   | `Message`       | `provider`, `responseModel ?? model` |
//! | `message`, toolResult, `message.usage`  | `ToolResult`    | none: [`UNATTRIBUTED_MODEL`]         |
//! | `usage`                                 | `Auxiliary`     | `provider`, `model` of the entry     |
//! | `compaction`, `usage`                   | `Compaction`    | none: [`UNATTRIBUTED_MODEL`]         |
//! | `branch_summary`, `usage`               | `BranchSummary` | none: [`UNATTRIBUTED_MODEL`]         |

use std::borrow::Cow;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokenmtr_core::{
    AgentId, DedupKey, EventKind, ModelId, ParsedEvent, ProfileId, ProviderId, SessionId,
    SignalKind, SkillName, SourceFileId, TokenCounts, ToolCall, UsdMicros, split_mcp_tool_name,
};

use super::header::{self, SessionHeader};
use crate::batch::{ParseBatch, ParsedSignal, ParsedToolCall, ParsedToolResult};
use crate::blocks::Blocks;
use crate::drift::LineClass;
use crate::error::AgentError;
use crate::source::SourceFile;

/// The model of events whose entry carries no model: tool results, compactions and branch
/// summaries (pi attributes their usage to a model internally but does not write it down).
pub const UNATTRIBUTED_MODEL: &str = "unattributed";

/// The tool pi loads a skill with: it reads the skill's [`SKILL_FILE`].
const READ_TOOL: &str = "read";
/// The file that defines a skill; its directory names the skill.
const SKILL_FILE: &str = "SKILL.md";

/// Per-file facts the line parser needs. Build one per source file with [`LineContext::new`].
#[derive(Debug, Clone)]
pub struct LineContext {
    profile: ProfileId,
    source_file: Option<SourceFileId>,
    session: Option<SessionId>,
    parent_session: Option<SessionId>,
    cwd: Option<Utf8PathBuf>,
    unattributed: ModelId,
}

impl LineContext {
    /// Derives the context of `file` from its `header` (see [`header::read`]). A file without a
    /// usable header takes its session id from its name (`<timestamp>_<id>.jsonl`, else the whole
    /// stem), and has no cwd and no parent.
    pub fn new(file: &SourceFile, header: Option<&SessionHeader>) -> Result<Self, AgentError> {
        Ok(Self {
            profile: file.profile.clone(),
            source_file: file.id,
            session: header
                .map(|header| header.id.clone())
                .or_else(|| header::session_id_from_file_name(&file.path))
                .or_else(|| {
                    file.path
                        .file_stem()
                        .and_then(|stem| SessionId::try_from(stem).ok())
                }),
            parent_session: header.and_then(|header| header.parent_session.clone()),
            cwd: header.and_then(|header| header.cwd.clone()),
            unattributed: ModelId::try_from(UNATTRIBUTED_MODEL)?,
        })
    }
}

/// Parses one session line into `out`. Never panics, whatever the bytes. The header line parses
/// as [`LineClass::NoUsage`]: [`SessionHeader::parse`] reads it.
///
/// Pushes, as the line allows:
/// - a usage event, for the usage-bearing entries in the module table;
/// - tool calls, for `toolCall` blocks of assistant messages, with the skill a `read` of a
///   `SKILL.md` loads;
/// - tool results, for toolResult messages;
/// - a signal, for `compaction` entries.
///
/// A usage block with no tokens and no cost (an aborted call) is [`LineClass::Ignored`].
pub fn parse_line(ctx: &LineContext, line: &[u8], out: &mut ParseBatch) -> LineClass {
    let entry: Entry<'_> = match serde_json::from_slice(line) {
        Ok(entry) => entry,
        Err(_) => return classify_failure(line),
    };
    let at = entry
        .timestamp
        .as_deref()
        .and_then(|text| text.parse::<jiff::Timestamp>().ok());
    match entry.kind.as_deref() {
        Some("message") => match &entry.message {
            Some(message) => message_entry(ctx, out, &entry, message, at),
            None => LineClass::NoUsage,
        },
        Some("usage") => match &entry.usage {
            Some(usage) => record(
                ctx,
                out,
                &entry,
                &Source {
                    kind: EventKind::Auxiliary,
                    provider: entry.provider.as_deref(),
                    model: entry.model.as_deref(),
                    response_id: None,
                },
                usage,
                at,
            ),
            None => LineClass::Unmapped,
        },
        Some("compaction") => {
            if let (Some(session), Some(at)) = (&ctx.session, at) {
                out.push_signal(ParsedSignal {
                    agent: AgentId::PI,
                    session: session.clone(),
                    kind: SignalKind::Compaction,
                    at,
                });
            }
            nested_usage(ctx, out, &entry, EventKind::Compaction, at)
        }
        Some("branch_summary") => nested_usage(ctx, out, &entry, EventKind::BranchSummary, at),
        _ => LineClass::NoUsage,
    }
}

/// A `compaction` or `branch_summary` entry: usage is optional and the entry names no model.
fn nested_usage(
    ctx: &LineContext,
    out: &mut ParseBatch,
    entry: &Entry<'_>,
    kind: EventKind,
    at: Option<jiff::Timestamp>,
) -> LineClass {
    match &entry.usage {
        Some(usage) => record(
            ctx,
            out,
            entry,
            &Source {
                kind,
                provider: None,
                model: None,
                response_id: None,
            },
            usage,
            at,
        ),
        None => LineClass::NoUsage,
    }
}

fn message_entry(
    ctx: &LineContext,
    out: &mut ParseBatch,
    entry: &Entry<'_>,
    message: &Message<'_>,
    at: Option<jiff::Timestamp>,
) -> LineClass {
    match message.role.as_deref() {
        Some("assistant") => {
            let (class, event_key) = match &message.usage {
                Some(usage) => {
                    let model = message
                        .response_model
                        .as_deref()
                        .filter(|model| !model.is_empty())
                        .or(message.model.as_deref());
                    record_with_key(
                        ctx,
                        out,
                        entry,
                        &Source {
                            kind: EventKind::Message,
                            provider: message.provider.as_deref(),
                            model,
                            response_id: message.response_id.as_deref(),
                        },
                        usage,
                        at,
                    )
                }
                None => (LineClass::NoUsage, None),
            };
            if let (Some(session), Some(at)) = (&ctx.session, at) {
                for block in message.content.0.iter().filter(|b| b.is("toolCall")) {
                    push_tool_call(out, block, session, at, event_key.as_ref());
                }
            }
            class
        }
        Some("toolResult") => {
            if let Some(id) = message.tool_call_id.as_deref().filter(|id| !id.is_empty()) {
                out.push_tool_result(ParsedToolResult {
                    agent: AgentId::PI,
                    tool_use_id: id.to_owned(),
                    is_error: message.is_error.unwrap_or(false),
                });
            }
            match &message.usage {
                Some(usage) => record(
                    ctx,
                    out,
                    entry,
                    &Source {
                        kind: EventKind::ToolResult,
                        provider: None,
                        model: None,
                        response_id: None,
                    },
                    usage,
                    at,
                ),
                None => LineClass::NoUsage,
            }
        }
        _ => LineClass::NoUsage,
    }
}

fn push_tool_call(
    out: &mut ParseBatch,
    block: &Block<'_>,
    session: &SessionId,
    at: jiff::Timestamp,
    event_key: Option<&DedupKey>,
) {
    let (Some(id), Some(name)) = (block.id.as_deref(), block.name.as_deref()) else {
        return;
    };
    if id.is_empty() || name.is_empty() {
        return;
    }
    let skill = if name == READ_TOOL {
        block.arguments.and_then(skill_of)
    } else {
        None
    };
    let name = split_mcp_tool_name(name);
    out.push_tool_call(ParsedToolCall {
        call: ToolCall {
            agent: AgentId::PI,
            tool_use_id: id.to_owned(),
            event: None,
            session: session.clone(),
            at,
            tool_name: name.tool_name.to_owned(),
            mcp_server: name.mcp_server.map(str::to_owned),
            is_error: None,
            skill,
        },
        event_key: event_key.cloned(),
    });
}

/// The skill a `read` call loads: the directory name of an `arguments.path` whose file name is
/// exactly [`SKILL_FILE`] (`/x/skills/foo/SKILL.md` is `foo`). The rest of the path is dropped.
/// Any other shape of `arguments`, a path without a parent directory or an invalid name yields
/// no skill.
fn skill_of(arguments: &RawValue) -> Option<SkillName> {
    let arguments: ReadArguments<'_> = serde_json::from_str(arguments.get()).ok()?;
    let path = Utf8Path::new(arguments.path.as_deref()?);
    if path.file_name() != Some(SKILL_FILE) {
        return None;
    }
    SkillName::try_from(path.parent()?.file_name()?.trim()).ok()
}

/// Where the identity of one usage block comes from.
struct Source<'a> {
    kind: EventKind,
    provider: Option<&'a str>,
    model: Option<&'a str>,
    response_id: Option<&'a str>,
}

fn record(
    ctx: &LineContext,
    out: &mut ParseBatch,
    entry: &Entry<'_>,
    source: &Source<'_>,
    usage: &Usage,
    at: Option<jiff::Timestamp>,
) -> LineClass {
    record_with_key(ctx, out, entry, source, usage, at).0
}

/// Maps and pushes the event of one usage block; also returns its dedup key when it pushed one.
fn record_with_key(
    ctx: &LineContext,
    out: &mut ParseBatch,
    entry: &Entry<'_>,
    source: &Source<'_>,
    usage: &Usage,
    at: Option<jiff::Timestamp>,
) -> (LineClass, Option<DedupKey>) {
    match map_event(ctx, entry, source, usage, at) {
        None => (LineClass::Unmapped, None),
        Some(Mapping::Empty) => (LineClass::Ignored, None),
        Some(Mapping::Event(event)) => {
            let key = event.dedup_key.clone();
            out.push_event(*event);
            (LineClass::Event, Some(key))
        }
    }
}

enum Mapping {
    /// No tokens and no cost: an aborted call.
    Empty,
    Event(Box<ParsedEvent>),
}

/// Maps a usage block to an event. `None` when a required field is missing or unusable: session,
/// timestamp, a dedup key, the model (when the source names one), a finite cost, or both `input`
/// and `output`.
fn map_event(
    ctx: &LineContext,
    entry: &Entry<'_>,
    source: &Source<'_>,
    usage: &Usage,
    at: Option<jiff::Timestamp>,
) -> Option<Mapping> {
    if usage.input.is_none() && usage.output.is_none() {
        return None;
    }
    let cache_write = usage.cache_write.unwrap_or(0);
    let cache_write_1h = usage.cache_write_1h.unwrap_or(0).min(cache_write);
    let output = usage.output.unwrap_or(0);
    let tokens = TokenCounts {
        input_uncached: usage.input.unwrap_or(0),
        cache_read: usage.cache_read.unwrap_or(0),
        cache_write_5m: cache_write - cache_write_1h,
        cache_write_1h,
        output,
        reasoning: usage.reasoning.unwrap_or(0).min(output),
    };
    let raw_cost = match usage.cost.as_ref().and_then(|cost| cost.total) {
        Some(total) => Some(UsdMicros::from_f64(total).ok()?),
        None => None,
    };
    // A negative cost is a pi bug, not a refund: drop it, so the event is estimated and flagged
    // approximate like a zero reported cost.
    let negative_cost = raw_cost.is_some_and(|cost| cost < UsdMicros::ZERO);
    let reported_cost = raw_cost.filter(|_| !negative_cost);
    if tokens == TokenCounts::default() && reported_cost.is_none_or(|cost| cost == UsdMicros::ZERO)
    {
        return Some(Mapping::Empty);
    }
    let model = match source.kind {
        EventKind::Message | EventKind::Auxiliary => {
            ModelId::try_from(source.model.filter(|model| !model.is_empty())?).ok()?
        }
        _ => ctx.unattributed.clone(),
    };
    Some(Mapping::Event(Box::new(ParsedEvent {
        agent: AgentId::PI,
        profile: ctx.profile.clone(),
        dedup_key: dedup_key(entry, source, &tokens, raw_cost)?,
        at: at?,
        session: ctx.session.clone()?,
        parent_session: ctx.parent_session.clone(),
        subagent: None,
        is_sidechain: false,
        cwd: ctx.cwd.clone(),
        git_branch: None,
        source_file: ctx.source_file,
        approximate: negative_cost,
        provider: source
            .provider
            .filter(|provider| !provider.is_empty())
            .and_then(|provider| ProviderId::try_from(provider).ok()),
        model,
        kind: source.kind,
        tokens,
        reported_cost,
        service_tier: None,
        modifiers: None,
    })))
}

/// The assistant `responseId`, else a key built from what pi never rewrites: the entry
/// timestamp text, the event kind, provider, model, token counts and cost in micro-USD, joined
/// with `|`.
///
/// - pi's v1 to v2 migration gives every entry a new random `id`, so an id-based key would count
///   a migrated file twice; the entry `timestamp` and the usage survive the rewrite verbatim.
/// - A forked session file copies the entry, so the copy dedups against the original.
/// - Neither the session nor the file is part of the key, for the same reason.
fn dedup_key(
    entry: &Entry<'_>,
    source: &Source<'_>,
    tokens: &TokenCounts,
    cost: Option<UsdMicros>,
) -> Option<DedupKey> {
    if let Some(id) = source.response_id.filter(|id| !id.is_empty()) {
        return DedupKey::try_from(id).ok();
    }
    let timestamp = entry.timestamp.as_deref().filter(|at| !at.is_empty())?;
    let kind = match source.kind {
        EventKind::Message => "message",
        EventKind::ToolResult => "tool_result",
        EventKind::Compaction => "compaction",
        EventKind::BranchSummary => "branch_summary",
        EventKind::Auxiliary => "auxiliary",
        EventKind::DailyAggregate => "daily_aggregate",
        EventKind::ExtraUsage => "extra_usage",
    };
    let TokenCounts {
        input_uncached,
        cache_read,
        cache_write_5m,
        cache_write_1h,
        output,
        reasoning,
    } = *tokens;
    let cost = cost.map_or_else(String::new, |cost| cost.get().to_string());
    DedupKey::try_from(format!(
        "{timestamp}|{kind}|{}|{}|{input_uncached}|{cache_read}|{cache_write_5m}|{cache_write_1h}|{output}|{reasoning}|{cost}",
        source.provider.unwrap_or_default(),
        source.model.unwrap_or_default(),
    ))
    .ok()
}

/// A line that did not deserialize: not JSON at all, or JSON of an unexpected shape. A usage-bearing
/// entry that mentions `usage` is a known usage-bearing line that failed to map (drift).
fn classify_failure(line: &[u8]) -> LineClass {
    let usage_bearing = memchr::memmem::find(line, b"usage").is_some();
    match serde_json::from_slice::<Probe<'_>>(line) {
        Err(_) => LineClass::Malformed { usage_bearing },
        Ok(probe)
            if usage_bearing
                && matches!(
                    probe.kind.as_deref(),
                    Some("message" | "usage" | "compaction" | "branch_summary")
                ) =>
        {
            LineClass::Unmapped
        }
        Ok(_) => LineClass::NoUsage,
    }
}

#[derive(Deserialize)]
struct Probe<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    /// `usage` entries only.
    #[serde(borrow)]
    provider: Option<Cow<'a, str>>,
    /// `usage` entries only.
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    /// `usage`, `compaction` and `branch_summary` entries.
    usage: Option<Usage>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    #[serde(borrow)]
    role: Option<Cow<'a, str>>,
    #[serde(borrow)]
    provider: Option<Cow<'a, str>>,
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    #[serde(rename = "responseModel", borrow)]
    response_model: Option<Cow<'a, str>>,
    #[serde(rename = "responseId", borrow)]
    response_id: Option<Cow<'a, str>>,
    #[serde(rename = "toolCallId", borrow)]
    tool_call_id: Option<Cow<'a, str>>,
    #[serde(rename = "isError")]
    is_error: Option<bool>,
    usage: Option<Usage>,
    #[serde(default, borrow)]
    content: Blocks<Block<'a>>,
}

/// pi's `Usage`. `cacheWrite1h` is a subset of `cacheWrite` and `reasoning` of `output`.
#[derive(Deserialize)]
struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    #[serde(rename = "cacheRead")]
    cache_read: Option<u64>,
    #[serde(rename = "cacheWrite")]
    cache_write: Option<u64>,
    #[serde(rename = "cacheWrite1h")]
    cache_write_1h: Option<u64>,
    reasoning: Option<u64>,
    cost: Option<Cost>,
}

#[derive(Deserialize)]
struct Cost {
    /// USD.
    total: Option<f64>,
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    /// The tool arguments, kept as borrowed raw JSON (no copy); only [`skill_of`] reads them,
    /// and only for a `read` call. Raw, so an odd shape never fails the line.
    #[serde(borrow)]
    arguments: Option<&'a RawValue>,
}

impl Block<'_> {
    fn is(&self, kind: &str) -> bool {
        self.kind.as_deref() == Some(kind)
    }
}

/// The arguments of a `read` call; every field but `path` is skipped.
#[derive(Deserialize)]
struct ReadArguments<'a> {
    #[serde(borrow)]
    path: Option<Cow<'a, str>>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// Parses one assistant message with usage and a `toolCall` block of `name` and `arguments`.
    fn parse_tool_call(name: &str, arguments: &Value) -> TestResult<(LineClass, Option<String>)> {
        let file = SourceFile::new(
            "/s/--p--/2026-01-01T00-00-00-000Z_sess.jsonl".into(),
            ProfileId::default_profile(),
        );
        let line = json!({"type": "message", "timestamp": "2026-03-01T00:00:00Z",
            "message": {"role": "assistant", "provider": "anthropic",
                "model": "claude-sonnet-4-5", "responseId": "r",
                "usage": {"input": 1, "output": 1},
                "content": [{"type": "toolCall", "id": "toolu_1", "name": name,
                    "arguments": arguments}]}})
        .to_string();
        let mut batch = ParseBatch::new();
        let class = parse_line(&LineContext::new(&file, None)?, line.as_bytes(), &mut batch);
        let [call] = batch.tool_calls() else {
            return Err("expected one tool call".into());
        };
        let skill = call.call.skill.as_ref().map(|s| s.as_str().to_owned());
        Ok((class, skill))
    }

    fn skill_of_read(path: &str) -> TestResult<Option<String>> {
        Ok(parse_tool_call("read", &json!({ "path": path, "offset": 1 }))?.1)
    }

    #[test]
    fn reading_a_skill_file_records_its_directory() -> TestResult {
        assert_eq!(
            skill_of_read("/Users/x/.agents/skills/add-dalec-image/SKILL.md")?.as_deref(),
            Some("add-dalec-image")
        );
        assert_eq!(
            skill_of_read(".agents/skills/foo/SKILL.md")?.as_deref(),
            Some("foo")
        );
        Ok(())
    }

    #[test]
    fn other_reads_and_tools_record_no_skill() -> TestResult {
        for path in [
            "SKILL.md",
            "/SKILL.md",
            "../SKILL.md",
            "/x/foo/skill.md",
            "/x/foo/SKILL.md.bak",
            "/x/foo/README.md",
            "",
        ] {
            assert_eq!(skill_of_read(path)?, None, "{path}");
        }
        let long = format!("/x/{}/SKILL.md", "a".repeat(129));
        assert_eq!(skill_of_read(&long)?, None);
        assert_eq!(skill_of_read("/x/a\u{1}b/SKILL.md")?, None);
        assert_eq!(
            parse_tool_call("edit", &json!({"path": "/x/foo/SKILL.md"}))?.1,
            None
        );
        Ok(())
    }

    #[test]
    fn odd_arguments_record_no_skill_and_keep_the_line() -> TestResult {
        for arguments in [json!("/x/foo/SKILL.md"), json!({"path": 3}), json!(null)] {
            assert_eq!(
                parse_tool_call("read", &arguments)?,
                (LineClass::Event, None),
                "{arguments}"
            );
        }
        Ok(())
    }
}
