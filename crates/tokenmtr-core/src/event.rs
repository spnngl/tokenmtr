//! Usage events, their token counts and costs, and tool-call metadata.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::ids::{
    AgentId, DedupKey, EventId, MachineId, ModelId, ProfileId, ProjectKey, ProviderId, SessionId,
    SourceFileId,
};
use crate::money::UsdMicros;
use crate::pricing::PricingVersion;

/// Disjoint token columns.
///
/// `reasoning` is informational: it is a subset of `output` and is never summed into totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct TokenCounts {
    /// Input tokens not served from cache and not written to it.
    pub input_uncached: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the 5-minute cache.
    pub cache_write_5m: u64,
    /// Input tokens written to the 1-hour cache.
    pub cache_write_1h: u64,
    /// Output tokens (including reasoning).
    pub output: u64,
    /// Reasoning tokens, a subset of `output`.
    pub reasoning: u64,
}

impl TokenCounts {
    /// Size of the prompt: every input column. Selects `LiteLLM` threshold tiers.
    #[must_use]
    pub const fn prompt_tokens(&self) -> u64 {
        self.input_uncached
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write_5m)
            .saturating_add(self.cache_write_1h)
    }

    /// `input_uncached + cache_read + cache_write_5m + cache_write_1h + output`.
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.prompt_tokens().saturating_add(self.output)
    }
}

/// Per-request billing modifiers Anthropic reports in `usage`: `speed`, `inference_geo` and
/// `server_tool_use.web_search_requests`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PriceModifiers {
    /// `speed == "fast"`: fast mode, billed at the entry's `fast` multiplier.
    pub fast: bool,
    /// Region the request ran in, lowercase. `None` for global routing or when not reported.
    pub inference_geo: Option<String>,
    /// Server-side web searches, billed per query.
    pub web_search_requests: u64,
}

/// An estimated API cost and the price that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Estimate {
    /// Estimated pay-as-you-go cost.
    pub usd: UsdMicros,
    /// Snapshot the estimate was computed from, or the hash of the override that priced it.
    pub pricing_version: PricingVersion,
}

/// Where an [`ApiCost`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// The agent reported it.
    Reported,
    /// Estimated from a pricing snapshot.
    Estimated,
    /// No rate matched.
    Unpriced,
}

/// Both costs stored for one event. Either may be absent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct EventCost {
    reported: Option<UsdMicros>,
    estimated: Option<Estimate>,
}

impl EventCost {
    /// Combines what the agent reported with what the catalog estimated.
    #[must_use]
    pub const fn new(reported: Option<UsdMicros>, estimated: Option<Estimate>) -> Self {
        Self {
            reported,
            estimated,
        }
    }

    /// The cost the agent reported.
    #[must_use]
    pub const fn reported(&self) -> Option<UsdMicros> {
        self.reported
    }

    /// The estimate from a pricing snapshot.
    #[must_use]
    pub const fn estimated(&self) -> Option<&Estimate> {
        self.estimated.as_ref()
    }

    /// `reported ?? estimated`, else unpriced.
    #[must_use]
    pub fn api_cost(&self) -> ApiCost {
        match (self.reported, &self.estimated) {
            (Some(usd), _) => ApiCost::Reported(usd),
            (None, Some(estimate)) => ApiCost::Estimated(estimate.clone()),
            (None, None) => ApiCost::Unpriced,
        }
    }
}

/// The pay-as-you-go cost of one event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiCost {
    /// Reported by the agent.
    Reported(UsdMicros),
    /// Estimated from a pricing snapshot.
    Estimated(Estimate),
    /// No rate matched.
    Unpriced,
}

impl ApiCost {
    /// The amount, if priced.
    #[must_use]
    pub const fn usd(&self) -> Option<UsdMicros> {
        match self {
            Self::Reported(usd) => Some(*usd),
            Self::Estimated(estimate) => Some(estimate.usd),
            Self::Unpriced => None,
        }
    }

    /// The `cost_source` column value.
    #[must_use]
    pub const fn source(&self) -> CostSource {
        match self {
            Self::Reported(_) => CostSource::Reported,
            Self::Estimated(_) => CostSource::Estimated,
            Self::Unpriced => CostSource::Unpriced,
        }
    }
}

/// What an agent line represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// An assistant message.
    Message,
    /// A tool result that carried usage.
    ToolResult,
    /// A compaction summary.
    Compaction,
    /// A branch summary.
    BranchSummary,
    /// A standalone usage entry.
    Auxiliary,
    /// A synthetic per-day, per-model row imported from a stats cache.
    DailyAggregate,
    /// A synthetic per-day row of Claude extra-usage spend (see [`crate::extra_usage`]): money
    /// without tokens, model, project or session.
    ExtraUsage,
}

impl EventKind {
    /// True for events of real calls (sessions, live rate, last activity): not the synthetic
    /// daily aggregates or extra-usage rows.
    #[must_use]
    pub const fn is_traffic(self) -> bool {
        match self {
            Self::Message
            | Self::ToolResult
            | Self::Compaction
            | Self::BranchSummary
            | Self::Auxiliary => true,
            Self::DailyAggregate | Self::ExtraUsage => false,
        }
    }
}

/// What an agent can do; the UI renders widgets from these flags, never from agent names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Exposes quota windows.
    pub quota: bool,
    /// Reports its own cost per event.
    pub reported_cost: bool,
    /// Can be covered by a subscription.
    pub subscription: bool,
}

/// One tool invocation: name and error flag, never arguments or results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Agent that invoked the tool.
    pub agent: AgentId,
    /// Tool-use id, unique per agent.
    pub tool_use_id: String,
    /// The usage event of the same message, when known.
    pub event: Option<EventId>,
    /// Owning session.
    pub session: SessionId,
    /// When the tool was invoked.
    pub at: jiff::Timestamp,
    /// Tool name (for MCP tools, the part after the server).
    pub tool_name: String,
    /// MCP server name for `mcp__<server>__<tool>` names.
    pub mcp_server: Option<String>,
    /// Set by the tool result; `None` until one is seen.
    pub is_error: Option<bool>,
}

/// A session-level signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// The API returned an error.
    ApiError,
    /// The agent compacted its context.
    Compaction,
}

/// A tool name split into its MCP server and tool parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolName<'a> {
    /// The MCP server, for `mcp__<server>__<tool>` names.
    pub mcp_server: Option<&'a str>,
    /// The tool name; the whole input for non-MCP names.
    pub tool_name: &'a str,
}

/// Splits `mcp__<server>__<tool>` at the first `__` after the prefix, so tool names that
/// themselves contain `__` stay intact. Anything else is returned unsplit.
#[must_use]
pub fn split_mcp_tool_name(name: &str) -> ToolName<'_> {
    let whole = ToolName {
        mcp_server: None,
        tool_name: name,
    };
    let Some(rest) = name.strip_prefix("mcp__") else {
        return whole;
    };
    match rest.split_once("__") {
        Some((server, tool)) if !server.is_empty() && !tool.is_empty() => ToolName {
            mcp_server: Some(server),
            tool_name: tool,
        },
        _ => whole,
    }
}

/// One billed LLM call: the atomic row of the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Globally unique id.
    pub id: EventId,
    /// Agent that wrote the event.
    pub agent: AgentId,
    /// Profile of that agent.
    pub profile: ProfileId,
    /// Per-agent dedup key.
    pub dedup_key: DedupKey,
    /// When the call happened (UTC instant).
    pub at: jiff::Timestamp,
    /// Owning session.
    pub session: SessionId,
    /// Parent session (pi forks).
    pub parent_session: Option<SessionId>,
    /// Subagent id, from the file stem.
    pub subagent: Option<String>,
    /// Whether the call ran in a sidechain.
    pub is_sidechain: bool,
    /// Working directory.
    pub cwd: Option<Utf8PathBuf>,
    /// Git branch at the time.
    pub git_branch: Option<String>,
    /// Resolved project.
    pub project: Option<ProjectKey>,
    /// Client label from `[[projects]]` rules.
    pub client: Option<String>,
    /// Source file the event came from.
    pub source_file: Option<SourceFileId>,
    /// True for `DailyAggregate` events and an `ExtraUsage` day holding a first observation.
    pub approximate: bool,
    /// Serving provider, if known.
    pub provider: Option<ProviderId>,
    /// Model as reported.
    pub model: ModelId,
    /// What the line represents.
    pub kind: EventKind,
    /// Token columns.
    pub tokens: TokenCounts,
    /// Reported and estimated costs.
    pub cost: EventCost,
    /// Service tier (selects `LiteLLM` `_priority` / `_flex` rates).
    pub service_tier: Option<String>,
}

/// What an agent parser produces: a usage event before identity (`EventId` needs the machine) and
/// estimation. Estimation happens afterwards, in one place (`pricing::estimate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedEvent {
    /// Agent that wrote the event.
    pub agent: AgentId,
    /// Profile of that agent.
    pub profile: ProfileId,
    /// Per-agent dedup key.
    pub dedup_key: DedupKey,
    /// When the call happened (UTC instant).
    pub at: jiff::Timestamp,
    /// Owning session.
    pub session: SessionId,
    /// Parent session (pi forks).
    pub parent_session: Option<SessionId>,
    /// Subagent id, from the file stem.
    pub subagent: Option<String>,
    /// Whether the call ran in a sidechain.
    pub is_sidechain: bool,
    /// Working directory.
    pub cwd: Option<Utf8PathBuf>,
    /// Git branch at the time.
    pub git_branch: Option<String>,
    /// Source file the event came from.
    pub source_file: Option<SourceFileId>,
    /// True for `DailyAggregate` events.
    pub approximate: bool,
    /// Serving provider, if known.
    pub provider: Option<ProviderId>,
    /// Model as reported.
    pub model: ModelId,
    /// What the line represents.
    pub kind: EventKind,
    /// Token columns.
    pub tokens: TokenCounts,
    /// Cost the agent reported, if it does.
    pub reported_cost: Option<UsdMicros>,
    /// Service tier.
    pub service_tier: Option<String>,
    /// Billing modifiers; `None` when the agent does not report them. They are not stored, so
    /// only the estimate made at ingest can use them.
    #[serde(default)]
    pub modifiers: Option<PriceModifiers>,
}

impl ParsedEvent {
    /// Finishes the event: derives its id and attaches the estimate. `project` and `client` are
    /// left empty for the ingestor to resolve.
    #[must_use]
    pub fn into_usage_event(self, machine: &MachineId, estimate: Option<Estimate>) -> UsageEvent {
        UsageEvent {
            id: EventId::new(machine, self.agent, &self.dedup_key),
            agent: self.agent,
            profile: self.profile,
            dedup_key: self.dedup_key,
            at: self.at,
            session: self.session,
            parent_session: self.parent_session,
            subagent: self.subagent,
            is_sidechain: self.is_sidechain,
            cwd: self.cwd,
            git_branch: self.git_branch,
            project: None,
            client: None,
            source_file: self.source_file,
            approximate: self.approximate,
            provider: self.provider,
            model: self.model,
            kind: self.kind,
            tokens: self.tokens,
            cost: EventCost::new(self.reported_cost, estimate),
            service_tier: self.service_tier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> TokenCounts {
        TokenCounts {
            input_uncached: 1,
            cache_read: 2,
            cache_write_5m: 3,
            cache_write_1h: 4,
            output: 5,
            reasoning: 5,
        }
    }

    #[test]
    fn total_tokens_excludes_reasoning() {
        assert_eq!(tokens().total_tokens(), 15);
        assert_eq!(tokens().prompt_tokens(), 10);
    }

    #[test]
    fn total_tokens_saturates() {
        let t = TokenCounts {
            output: u64::MAX,
            input_uncached: 1,
            ..TokenCounts::default()
        };
        assert_eq!(t.total_tokens(), u64::MAX);
    }

    #[test]
    fn api_cost_prefers_reported() {
        let estimate = Estimate {
            usd: UsdMicros::new(9),
            pricing_version: PricingVersion::of_body(b"x"),
        };
        let both = EventCost::new(Some(UsdMicros::new(5)), Some(estimate.clone()));
        assert_eq!(both.api_cost(), ApiCost::Reported(UsdMicros::new(5)));
        let est = EventCost::new(None, Some(estimate));
        assert_eq!(est.api_cost().usd(), Some(UsdMicros::new(9)));
        assert_eq!(est.api_cost().source(), CostSource::Estimated);
        assert_eq!(EventCost::default().api_cost(), ApiCost::Unpriced);
        assert_eq!(EventCost::default().api_cost().usd(), None);
    }

    #[test]
    fn mcp_names_split_on_the_first_separator() {
        let t = split_mcp_tool_name("mcp__github__create_issue");
        assert_eq!(
            (t.mcp_server, t.tool_name),
            (Some("github"), "create_issue")
        );
        let t = split_mcp_tool_name("mcp__srv__do__thing");
        assert_eq!((t.mcp_server, t.tool_name), (Some("srv"), "do__thing"));
    }

    #[test]
    fn non_mcp_and_malformed_names_stay_whole() {
        for name in [
            "Bash",
            "mcp__server",
            "mcp____tool",
            "mcp__server__",
            "xmcp__a__b",
        ] {
            let t = split_mcp_tool_name(name);
            assert_eq!((t.mcp_server, t.tool_name), (None, name), "{name}");
        }
    }
}
