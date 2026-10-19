//! The metric catalog: one source of truth for every name, type, help text and label set.
//!
//! The encoders, the generated table in `docs/metrics.md` and the tests all read it, so a
//! family cannot exist in one place and not another.

/// What a family measures over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A total that only grows (barring the documented drops).
    Counter,
    /// A value read at snapshot time.
    Gauge,
}

/// One metric family. Variants are declared in name order, so the derived `Ord` sorts the
/// output the way the text format wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    /// `tokenmtr_build_info`.
    BuildInfo,
    /// `tokenmtr_events_total`.
    Events,
    /// `tokenmtr_quota_fetched_at_timestamp_seconds`.
    QuotaFetchedAt,
    /// `tokenmtr_quota_resets_at_timestamp_seconds`.
    QuotaResetsAt,
    /// `tokenmtr_quota_used_ratio`.
    QuotaUsedRatio,
    /// `tokenmtr_reasoning_tokens_total`.
    ReasoningTokens,
    /// `tokenmtr_series`.
    Series,
    /// `tokenmtr_session_events_total`.
    SessionEvents,
    /// `tokenmtr_session_reasoning_tokens_total`.
    SessionReasoningTokens,
    /// `tokenmtr_session_tokens_total`.
    SessionTokens,
    /// `tokenmtr_session_tool_calls_total`.
    SessionToolCalls,
    /// `tokenmtr_snapshot_timestamp_seconds`.
    SnapshotTimestamp,
    /// `tokenmtr_tokens_total`.
    Tokens,
    /// `tokenmtr_tool_calls_total`.
    ToolCalls,
}

/// The `type` label values of the token families, in the order of [`TOKEN_TYPES`].
pub const TOKEN_TYPES: [&str; 5] = [
    "input_uncached",
    "cache_read",
    "cache_write_5m",
    "cache_write_1h",
    "output",
];

/// Label names of the usage families (P5).
const USAGE: [&str; 8] = [
    "account", "agent", "billing", "client", "model", "profile", "project", "provider",
];
const USAGE_TYPE: [&str; 9] = [
    "account", "agent", "billing", "client", "model", "profile", "project", "provider", "type",
];
const USAGE_KIND: [&str; 9] = [
    "account", "agent", "billing", "client", "kind", "model", "profile", "project", "provider",
];
const SESSION: [&str; 10] = [
    "account",
    "agent",
    "billing",
    "client",
    "git_branch",
    "model",
    "profile",
    "project",
    "provider",
    "session_id",
];
const SESSION_TYPE: [&str; 11] = [
    "account",
    "agent",
    "billing",
    "client",
    "git_branch",
    "model",
    "profile",
    "project",
    "provider",
    "session_id",
    "type",
];
const SESSION_KIND: [&str; 11] = [
    "account",
    "agent",
    "billing",
    "client",
    "git_branch",
    "kind",
    "model",
    "profile",
    "project",
    "provider",
    "session_id",
];
const TOOL: [&str; 5] = ["agent", "error", "mcp_server", "profile", "tool"];
const SESSION_TOOL: [&str; 6] = [
    "agent",
    "error",
    "mcp_server",
    "profile",
    "session_id",
    "tool",
];
const QUOTA: [&str; 4] = ["account", "agent", "profile", "window"];

impl Family {
    /// Every family, in name order.
    pub const ALL: [Self; 14] = [
        Self::BuildInfo,
        Self::Events,
        Self::QuotaFetchedAt,
        Self::QuotaResetsAt,
        Self::QuotaUsedRatio,
        Self::ReasoningTokens,
        Self::Series,
        Self::SessionEvents,
        Self::SessionReasoningTokens,
        Self::SessionTokens,
        Self::SessionToolCalls,
        Self::SnapshotTimestamp,
        Self::Tokens,
        Self::ToolCalls,
    ];

    /// The sample name, with `_total` on counters.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BuildInfo => "tokenmtr_build_info",
            Self::Events => "tokenmtr_events_total",
            Self::QuotaFetchedAt => "tokenmtr_quota_fetched_at_timestamp_seconds",
            Self::QuotaResetsAt => "tokenmtr_quota_resets_at_timestamp_seconds",
            Self::QuotaUsedRatio => "tokenmtr_quota_used_ratio",
            Self::ReasoningTokens => "tokenmtr_reasoning_tokens_total",
            Self::Series => "tokenmtr_series",
            Self::SessionEvents => "tokenmtr_session_events_total",
            Self::SessionReasoningTokens => "tokenmtr_session_reasoning_tokens_total",
            Self::SessionTokens => "tokenmtr_session_tokens_total",
            Self::SessionToolCalls => "tokenmtr_session_tool_calls_total",
            Self::SnapshotTimestamp => "tokenmtr_snapshot_timestamp_seconds",
            Self::Tokens => "tokenmtr_tokens_total",
            Self::ToolCalls => "tokenmtr_tool_calls_total",
        }
    }

    /// Counter or gauge.
    #[must_use]
    pub const fn kind(self) -> Kind {
        match self {
            Self::BuildInfo
            | Self::QuotaFetchedAt
            | Self::QuotaResetsAt
            | Self::QuotaUsedRatio
            | Self::Series
            | Self::SnapshotTimestamp => Kind::Gauge,
            Self::Events
            | Self::ReasoningTokens
            | Self::SessionEvents
            | Self::SessionReasoningTokens
            | Self::SessionTokens
            | Self::SessionToolCalls
            | Self::Tokens
            | Self::ToolCalls => Kind::Counter,
        }
    }

    /// The `# HELP` text.
    #[must_use]
    pub const fn help(self) -> &'static str {
        match self {
            Self::BuildInfo => "Constant 1; the label carries the TokenMtr version.",
            Self::Events => "Usage events recorded, by kind.",
            Self::QuotaFetchedAt => {
                "When the quota poll behind the quota gauges ran, in Unix seconds."
            }
            Self::QuotaResetsAt => "When the quota window resets, in Unix seconds.",
            Self::QuotaUsedRatio => "Share of the quota window used, from 0 to 1.",
            Self::ReasoningTokens => "Reasoning tokens, a part of the output tokens.",
            Self::Series => "Series in this output, this one included.",
            Self::SessionEvents => "Usage events of a session active in the session window.",
            Self::SessionReasoningTokens => {
                "Reasoning tokens of a session active in the session window."
            }
            Self::SessionTokens => "Tokens of a session active in the session window, by type.",
            Self::SessionToolCalls => "Tool calls of a session active in the session window.",
            Self::SnapshotTimestamp => {
                "When this snapshot of the store was computed, in Unix seconds."
            }
            Self::Tokens => "Tokens used, by type.",
            Self::ToolCalls => "Tool calls, by tool.",
        }
    }

    /// Label names, sorted.
    #[must_use]
    pub const fn labels(self) -> &'static [&'static str] {
        match self {
            Self::BuildInfo => &["version"],
            Self::Events => &USAGE_KIND,
            Self::QuotaFetchedAt | Self::QuotaResetsAt | Self::QuotaUsedRatio => &QUOTA,
            Self::ReasoningTokens => &USAGE,
            Self::Series | Self::SnapshotTimestamp => &[],
            Self::SessionEvents => &SESSION_KIND,
            Self::SessionReasoningTokens => &SESSION,
            Self::SessionTokens => &SESSION_TYPE,
            Self::SessionToolCalls => &SESSION_TOOL,
            Self::Tokens => &USAGE_TYPE,
            Self::ToolCalls => &TOOL,
        }
    }

    /// Where the family comes from and what its labels mean, for `docs/metrics.md`.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::BuildInfo => "the workspace version",
            Self::Events | Self::ReasoningTokens | Self::Tokens => {
                "`usage_event` rows, all history"
            }
            Self::QuotaFetchedAt | Self::QuotaResetsAt | Self::QuotaUsedRatio => {
                "the newest successful quota poll of each profile"
            }
            Self::Series | Self::SnapshotTimestamp => "this output",
            Self::SessionEvents
            | Self::SessionReasoningTokens
            | Self::SessionTokens
            | Self::SessionToolCalls => "sessions with an event inside `[metrics].session_window`",
            Self::ToolCalls => "`tool_call` rows, all history",
        }
    }
}

/// Labels a redaction can hash or drop. `cwd` is not among them: no metric carries it.
pub const REDACTABLE: [&str; 5] = ["account", "client", "git_branch", "project", "session_id"];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn valid_name(name: &str, colon: bool) -> bool {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || (colon && c == ':'))
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || (colon && c == ':'))
    }

    #[test]
    fn families_are_listed_in_name_order() {
        let names: Vec<_> = Family::ALL.iter().map(|f| f.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        let mut by_ord = Family::ALL.to_vec();
        by_ord.sort_unstable();
        assert_eq!(by_ord, Family::ALL);
    }

    #[test]
    fn names_and_labels_are_valid_prometheus() {
        for family in Family::ALL {
            assert!(valid_name(family.name(), true), "{}", family.name());
            assert_eq!(
                family.kind() == Kind::Counter,
                family.name().ends_with("_total"),
                "{}",
                family.name()
            );
            let labels = family.labels();
            assert!(labels.is_sorted(), "{}", family.name());
            for label in labels {
                assert!(
                    valid_name(label, false) && !label.starts_with("__"),
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn config_reserves_exactly_the_catalog_labels() {
        let used: BTreeSet<_> = Family::ALL
            .iter()
            .flat_map(|f| f.labels().iter().copied())
            .collect();
        let reserved: BTreeSet<_> = tokenmtr_config::METRIC_LABELS.iter().copied().collect();
        assert_eq!(used, reserved);
    }

    #[test]
    fn names_use_the_product_prefix() {
        let prefix = format!("{}_", tokenmtr_core::brand::CLI_NAME);
        for family in Family::ALL {
            assert!(family.name().starts_with(&prefix), "{}", family.name());
        }
    }

    #[test]
    fn only_catalog_labels_are_redactable() {
        let used: BTreeSet<_> = Family::ALL
            .iter()
            .flat_map(|f| f.labels().iter().copied())
            .collect();
        for label in REDACTABLE {
            assert!(used.contains(label), "{label}");
        }
    }
}
