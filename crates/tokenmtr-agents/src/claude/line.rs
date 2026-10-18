//! Claude Code line parser (§5, D32, D48): borrowed, partial serde structs. Fields we do not
//! read are skipped without allocating, so large tool outputs are scanned but never copied.

use std::borrow::Cow;

use camino::Utf8PathBuf;
use serde::Deserialize;
use tokenmtr_core::{
    AgentId, DedupKey, EventKind, ModelId, ParsedEvent, ProfileId, ProviderId, SessionId,
    SignalKind, SourceFileId, TokenCounts, ToolCall, split_mcp_tool_name,
};

use crate::batch::{ParseBatch, ParsedSignal, ParsedToolCall, ParsedToolResult};
use crate::blocks::Blocks;
use crate::drift::LineClass;
use crate::error::AgentError;
use crate::source::SourceFile;

/// The model Claude Code writes on lines it generated itself (API errors, interrupts).
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Per-file facts the line parser needs. Build one per source file with [`LineContext::new`].
#[derive(Debug, Clone)]
pub struct LineContext {
    profile: ProfileId,
    provider: ProviderId,
    subagent: Option<String>,
    fallback_session: Option<SessionId>,
    source_file: Option<SourceFileId>,
}

impl LineContext {
    /// Derives the context of `file`:
    /// - `<session>/subagents/agent-*.jsonl`: the subagent id is the file stem and the fallback
    ///   session is the `<session>` directory, so the subagent folds into its parent session.
    /// - Any other file: the fallback session is the file stem.
    ///
    /// The fallback is used only when a line has no `sessionId`.
    pub fn new(file: &SourceFile) -> Result<Self, AgentError> {
        let stem = file.path.file_stem();
        let parent = file.path.parent();
        let in_subagents = parent.and_then(|dir| dir.file_name()) == Some("subagents");
        let (subagent, session_name) = if in_subagents {
            let session_dir = parent
                .and_then(|dir| dir.parent())
                .and_then(|dir| dir.file_name());
            (stem.map(str::to_owned), session_dir)
        } else {
            (None, stem)
        };
        Ok(Self {
            profile: file.profile.clone(),
            provider: ProviderId::try_from("anthropic")?,
            subagent,
            fallback_session: session_name.and_then(|name| SessionId::try_from(name).ok()),
            source_file: file.id,
        })
    }
}

/// What [`parse_line`] made of one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineReport<'a> {
    /// How the line is counted in the drift stats.
    pub class: LineClass,
    /// The Claude Code `version` the line declares, when it has one.
    pub version: Option<Cow<'a, str>>,
}

/// Parses one transcript line into `out`. Never panics, whatever the bytes.
///
/// Pushes, as the line allows:
/// - a usage event, for `type == "assistant"` lines with `message.usage` (not `<synthetic>`);
/// - tool calls, for `tool_use` blocks of assistant lines;
/// - tool results, for `tool_result` blocks of user lines;
/// - a signal, for `isApiErrorMessage` and `isCompactSummary` lines.
pub fn parse_line<'a>(ctx: &LineContext, line: &'a [u8], out: &mut ParseBatch) -> LineReport<'a> {
    let parsed: Line<'a> = match serde_json::from_slice(line) {
        Ok(parsed) => parsed,
        Err(_) => {
            return LineReport {
                class: classify_failure(line),
                version: None,
            };
        }
    };
    let is_assistant = parsed.kind.as_deref() == Some("assistant");
    let at = parsed
        .timestamp
        .as_deref()
        .and_then(|text| text.parse::<jiff::Timestamp>().ok());
    let session = parsed
        .session_id
        .as_deref()
        .and_then(|id| SessionId::try_from(id).ok())
        .or_else(|| ctx.fallback_session.clone());
    let message = parsed.message.as_ref();

    let mut class = LineClass::NoUsage;
    let mut event_key = None;
    let usage_bearing = message
        .filter(|_| is_assistant)
        .and_then(|message| message.usage.as_ref().map(|usage| (message, usage)));
    if let Some((message, usage)) = usage_bearing {
        if message.model.as_deref() == Some(SYNTHETIC_MODEL) {
            class = LineClass::Ignored;
        } else if let Some(event) = map_event(ctx, &parsed, message, usage, at, session.as_ref()) {
            event_key = Some(event.dedup_key.clone());
            out.push_event(event);
            class = LineClass::Event;
        } else {
            class = LineClass::Unmapped;
        }
    }

    if let (Some(at), Some(session)) = (at, session) {
        if is_assistant && parsed.is_api_error_message == Some(true) {
            push_signal(out, &session, at, SignalKind::ApiError);
        }
        if parsed.is_compact_summary == Some(true) {
            push_signal(out, &session, at, SignalKind::Compaction);
        }
        if is_assistant && let Some(message) = message {
            for block in message.content.0.iter().filter(|b| b.is("tool_use")) {
                push_tool_call(out, block, &session, at, event_key.as_ref());
            }
        }
    }
    if parsed.kind.as_deref() == Some("user")
        && let Some(message) = message
    {
        for block in message.content.0.iter().filter(|b| b.is("tool_result")) {
            if let Some(id) = block.tool_use_id.as_deref().filter(|id| !id.is_empty()) {
                out.push_tool_result(ParsedToolResult {
                    agent: AgentId::CLAUDE,
                    tool_use_id: id.to_owned(),
                    is_error: block.is_error.unwrap_or(false),
                });
            }
        }
    }
    LineReport {
        class,
        version: parsed.version,
    }
}

fn push_signal(out: &mut ParseBatch, session: &SessionId, at: jiff::Timestamp, kind: SignalKind) {
    out.push_signal(ParsedSignal {
        agent: AgentId::CLAUDE,
        session: session.clone(),
        kind,
        at,
    });
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
    let name = split_mcp_tool_name(name);
    out.push_tool_call(ParsedToolCall {
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: id.to_owned(),
            event: None,
            session: session.clone(),
            at,
            tool_name: name.tool_name.to_owned(),
            mcp_server: name.mcp_server.map(str::to_owned),
            is_error: None,
        },
        event_key: event_key.cloned(),
    });
}

/// Maps an assistant line with usage to an event. `None` when a required field is missing or
/// unusable: model, timestamp, session, a dedup key, or both `input_tokens` and `output_tokens`.
fn map_event(
    ctx: &LineContext,
    line: &Line<'_>,
    message: &Message<'_>,
    usage: &Usage<'_>,
    at: Option<jiff::Timestamp>,
    session: Option<&SessionId>,
) -> Option<ParsedEvent> {
    if usage.input_tokens.is_none() && usage.output_tokens.is_none() {
        return None;
    }
    let (five_minute, one_hour) = match &usage.cache_creation {
        Some(split) if split.ephemeral_5m.is_some() || split.ephemeral_1h.is_some() => (
            split.ephemeral_5m.unwrap_or(0),
            split.ephemeral_1h.unwrap_or(0),
        ),
        _ => (usage.cache_creation_input_tokens.unwrap_or(0), 0),
    };
    let dedup_key = dedup_key(line, message)?;
    Some(ParsedEvent {
        agent: AgentId::CLAUDE,
        profile: ctx.profile.clone(),
        dedup_key,
        at: at?,
        session: session?.clone(),
        parent_session: None,
        subagent: ctx.subagent.clone(),
        is_sidechain: line.is_sidechain.unwrap_or(false) || ctx.subagent.is_some(),
        cwd: line
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.is_empty())
            .map(Utf8PathBuf::from),
        git_branch: line
            .git_branch
            .as_deref()
            .filter(|branch| !branch.is_empty())
            .map(str::to_owned),
        source_file: ctx.source_file,
        approximate: false,
        provider: Some(ctx.provider.clone()),
        model: ModelId::try_from(message.model.as_deref()?).ok()?,
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: usage.input_tokens.unwrap_or(0),
            cache_read: usage.cache_read_input_tokens.unwrap_or(0),
            cache_write_5m: five_minute,
            cache_write_1h: one_hour,
            output: usage.output_tokens.unwrap_or(0),
            reasoning: 0,
        },
        reported_cost: None,
        service_tier: usage
            .service_tier
            .as_deref()
            .filter(|tier| !tier.is_empty())
            .map(str::to_owned),
    })
}

/// `"{message.id}|{requestId}"`, else `uuid` (D12).
fn dedup_key(line: &Line<'_>, message: &Message<'_>) -> Option<DedupKey> {
    let key = match message.id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!("{id}|{}", line.request_id.as_deref().unwrap_or_default()),
        None => line.uuid.as_deref()?.to_owned(),
    };
    DedupKey::try_from(key).ok()
}

/// A line that did not deserialize: not JSON at all, or JSON of an unexpected shape. An assistant
/// line that mentions `usage` is a known usage-bearing line that failed to map (drift).
fn classify_failure(line: &[u8]) -> LineClass {
    let usage_bearing = memchr::memmem::find(line, b"usage").is_some();
    match serde_json::from_slice::<Probe<'_>>(line) {
        Err(_) => LineClass::Malformed { usage_bearing },
        Ok(probe) if usage_bearing && probe.kind.as_deref() == Some("assistant") => {
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
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    uuid: Option<Cow<'a, str>>,
    #[serde(rename = "requestId", borrow)]
    request_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    cwd: Option<Cow<'a, str>>,
    #[serde(rename = "gitBranch", borrow)]
    git_branch: Option<Cow<'a, str>>,
    #[serde(borrow)]
    version: Option<Cow<'a, str>>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error_message: Option<bool>,
    #[serde(rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    #[serde(borrow)]
    usage: Option<Usage<'a>>,
    #[serde(default, borrow)]
    content: Blocks<Block<'a>>,
}

#[derive(Deserialize)]
struct Usage<'a> {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
    #[serde(borrow)]
    service_tier: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(rename = "ephemeral_5m_input_tokens")]
    ephemeral_5m: Option<u64>,
    #[serde(rename = "ephemeral_1h_input_tokens")]
    ephemeral_1h: Option<u64>,
}

/// One content block. Every other field (`input`, `content`, `text`, …) is skipped. A struct
/// with optional fields, not a tagged enum: serde buffers the whole value of a tagged enum.
#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow)]
    tool_use_id: Option<Cow<'a, str>>,
    is_error: Option<bool>,
}

impl Block<'_> {
    fn is(&self, kind: &str) -> bool {
        self.kind.as_deref() == Some(kind)
    }
}
