//! The metric catalog: one source of truth for every name, type, help text and label set.
//!
//! The encoders, the generated table in `docs/metrics.md` and the tests all read it, so a
//! family cannot exist in one place and not another.

use std::fmt::Write as _;

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
    /// `tokenmtr_account_info`.
    AccountInfo,
    /// `tokenmtr_build_info`.
    BuildInfo,
    /// `tokenmtr_events_total`.
    Events,
    /// `tokenmtr_last_event_timestamp_seconds`.
    LastEventAt,
    /// `tokenmtr_machine_info`.
    MachineInfo,
    /// `tokenmtr_parse_issues`.
    ParseIssues,
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
    /// `tokenmtr_session_info`.
    SessionInfo,
    /// `tokenmtr_session_last_event_timestamp_seconds`.
    SessionLastEventAt,
    /// `tokenmtr_session_reasoning_tokens_total`.
    SessionReasoningTokens,
    /// `tokenmtr_session_signals_total`.
    SessionSignals,
    /// `tokenmtr_session_skill_uses_total`.
    SessionSkillUses,
    /// `tokenmtr_session_start_timestamp_seconds`.
    SessionStartAt,
    /// `tokenmtr_session_tokens_total`.
    SessionTokens,
    /// `tokenmtr_session_tool_calls_total`.
    SessionToolCalls,
    /// `tokenmtr_signals_total`.
    Signals,
    /// `tokenmtr_skill_uses_total`.
    SkillUses,
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
const USAGE: [&str; 10] = [
    "account",
    "agent",
    "billing",
    "client",
    "model",
    "profile",
    "project",
    "provider",
    "service_tier",
    "sidechain",
];
const USAGE_TYPE: [&str; 11] = [
    "account",
    "agent",
    "billing",
    "client",
    "model",
    "profile",
    "project",
    "provider",
    "service_tier",
    "sidechain",
    "type",
];
const USAGE_KIND: [&str; 11] = [
    "account",
    "agent",
    "billing",
    "client",
    "kind",
    "model",
    "profile",
    "project",
    "provider",
    "service_tier",
    "sidechain",
];
/// A session family carries only what can change within a session; `tokenmtr_session_info`
/// links its `session_id` to the rest (agent, profile, account, machine).
const SESSION: [&str; 9] = [
    "billing",
    "client",
    "git_branch",
    "model",
    "project",
    "provider",
    "service_tier",
    "session_id",
    "sidechain",
];
const SESSION_TYPE: [&str; 10] = [
    "billing",
    "client",
    "git_branch",
    "model",
    "project",
    "provider",
    "service_tier",
    "session_id",
    "sidechain",
    "type",
];
const SESSION_KIND: [&str; 10] = [
    "billing",
    "client",
    "git_branch",
    "kind",
    "model",
    "project",
    "provider",
    "service_tier",
    "session_id",
    "sidechain",
];
const SESSION_INFO: [&str; 5] = ["account", "agent", "machine_id", "profile", "session_id"];
const TOOL: [&str; 5] = ["agent", "error", "mcp_server", "profile", "tool"];
const SESSION_TOOL: [&str; 4] = ["error", "mcp_server", "session_id", "tool"];
const SKILL: [&str; 3] = ["agent", "profile", "skill"];
const SESSION_SKILL: [&str; 2] = ["session_id", "skill"];
const SIGNAL: [&str; 3] = ["agent", "kind", "profile"];
const SESSION_SIGNAL: [&str; 2] = ["kind", "session_id"];
const QUOTA: [&str; 4] = ["account", "agent", "profile", "window"];
const ACCOUNT: [&str; 5] = ["account", "agent", "plan", "profile", "rate_limit_tier"];
const MACHINE: [&str; 10] = [
    "arch",
    "kernel_version",
    "machine_id",
    "machine_name",
    "os",
    "os_build",
    "os_id",
    "os_name",
    "os_version",
    "version",
];

impl Family {
    /// Every family, in name order.
    pub const ALL: [Self; 25] = [
        Self::AccountInfo,
        Self::BuildInfo,
        Self::Events,
        Self::LastEventAt,
        Self::MachineInfo,
        Self::ParseIssues,
        Self::QuotaFetchedAt,
        Self::QuotaResetsAt,
        Self::QuotaUsedRatio,
        Self::ReasoningTokens,
        Self::Series,
        Self::SessionEvents,
        Self::SessionInfo,
        Self::SessionLastEventAt,
        Self::SessionReasoningTokens,
        Self::SessionSignals,
        Self::SessionSkillUses,
        Self::SessionStartAt,
        Self::SessionTokens,
        Self::SessionToolCalls,
        Self::Signals,
        Self::SkillUses,
        Self::SnapshotTimestamp,
        Self::Tokens,
        Self::ToolCalls,
    ];

    /// The sample name, with `_total` on counters.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AccountInfo => "tokenmtr_account_info",
            Self::BuildInfo => "tokenmtr_build_info",
            Self::Events => "tokenmtr_events_total",
            Self::LastEventAt => "tokenmtr_last_event_timestamp_seconds",
            Self::MachineInfo => "tokenmtr_machine_info",
            Self::ParseIssues => "tokenmtr_parse_issues",
            Self::QuotaFetchedAt => "tokenmtr_quota_fetched_at_timestamp_seconds",
            Self::QuotaResetsAt => "tokenmtr_quota_resets_at_timestamp_seconds",
            Self::QuotaUsedRatio => "tokenmtr_quota_used_ratio",
            Self::ReasoningTokens => "tokenmtr_reasoning_tokens_total",
            Self::Series => "tokenmtr_series",
            Self::SessionEvents => "tokenmtr_session_events_total",
            Self::SessionInfo => "tokenmtr_session_info",
            Self::SessionLastEventAt => "tokenmtr_session_last_event_timestamp_seconds",
            Self::SessionReasoningTokens => "tokenmtr_session_reasoning_tokens_total",
            Self::SessionSignals => "tokenmtr_session_signals_total",
            Self::SessionSkillUses => "tokenmtr_session_skill_uses_total",
            Self::SessionStartAt => "tokenmtr_session_start_timestamp_seconds",
            Self::SessionTokens => "tokenmtr_session_tokens_total",
            Self::SessionToolCalls => "tokenmtr_session_tool_calls_total",
            Self::Signals => "tokenmtr_signals_total",
            Self::SkillUses => "tokenmtr_skill_uses_total",
            Self::SnapshotTimestamp => "tokenmtr_snapshot_timestamp_seconds",
            Self::Tokens => "tokenmtr_tokens_total",
            Self::ToolCalls => "tokenmtr_tool_calls_total",
        }
    }

    /// Counter or gauge.
    #[must_use]
    pub const fn kind(self) -> Kind {
        match self {
            Self::AccountInfo
            | Self::BuildInfo
            | Self::LastEventAt
            | Self::MachineInfo
            | Self::ParseIssues
            | Self::QuotaFetchedAt
            | Self::QuotaResetsAt
            | Self::QuotaUsedRatio
            | Self::Series
            | Self::SessionInfo
            | Self::SessionLastEventAt
            | Self::SessionStartAt
            | Self::SnapshotTimestamp => Kind::Gauge,
            Self::Events
            | Self::ReasoningTokens
            | Self::SessionEvents
            | Self::SessionReasoningTokens
            | Self::SessionSignals
            | Self::SessionSkillUses
            | Self::SessionTokens
            | Self::SessionToolCalls
            | Self::Signals
            | Self::SkillUses
            | Self::Tokens
            | Self::ToolCalls => Kind::Counter,
        }
    }

    /// The `# HELP` text.
    #[must_use]
    pub const fn help(self) -> &'static str {
        match self {
            Self::AccountInfo => {
                "Constant 1; the labels carry the plan and rate-limit tier the agent reports."
            }
            Self::BuildInfo => "Constant 1; the label carries the TokenMtr version.",
            Self::Events => "Usage events recorded, by kind.",
            Self::LastEventAt => "When the newest usage event happened, in Unix seconds.",
            Self::MachineInfo => {
                "Constant 1; the labels carry the machine, its operating system and the TokenMtr \
                 build."
            }
            Self::ParseIssues => "Open parse issues: agent files TokenMtr could not read.",
            Self::QuotaFetchedAt => {
                "When the quota poll behind the quota gauges ran, in Unix seconds."
            }
            Self::QuotaResetsAt => "When the quota window resets, in Unix seconds.",
            Self::QuotaUsedRatio => "Share of the quota window used, from 0 to 1.",
            Self::ReasoningTokens => "Reasoning tokens, a part of the output tokens.",
            Self::Series => "Series in this output, this one included.",
            Self::SessionEvents => "Usage events of a session active in the session window.",
            Self::SessionInfo => {
                "Constant 1; links an active session to its agent, profile, account and machine."
            }
            Self::SessionLastEventAt => {
                "When the newest usage event of an active session happened, in Unix seconds."
            }
            Self::SessionReasoningTokens => {
                "Reasoning tokens of a session active in the session window."
            }
            Self::SessionSignals => {
                "API errors and context compactions of a session active in the session window."
            }
            Self::SessionSkillUses => "Skills loaded by a session active in the session window.",
            Self::SessionStartAt => {
                "When the first usage event of an active session happened, in Unix seconds."
            }
            Self::SessionTokens => "Tokens of a session active in the session window, by type.",
            Self::SessionToolCalls => "Tool calls of a session active in the session window.",
            Self::Signals => "API errors and context compactions, by kind.",
            Self::SkillUses => "Skills loaded, by skill.",
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
            Self::AccountInfo => &ACCOUNT,
            Self::BuildInfo => &["version"],
            Self::Events => &USAGE_KIND,
            Self::LastEventAt => &["agent", "profile"],
            Self::MachineInfo => &MACHINE,
            Self::ParseIssues => &["agent", "kind"],
            Self::QuotaFetchedAt | Self::QuotaResetsAt | Self::QuotaUsedRatio => &QUOTA,
            Self::ReasoningTokens => &USAGE,
            Self::Series | Self::SnapshotTimestamp => &[],
            Self::SessionEvents => &SESSION_KIND,
            Self::SessionInfo => &SESSION_INFO,
            Self::SessionLastEventAt | Self::SessionStartAt => &["session_id"],
            Self::SessionReasoningTokens => &SESSION,
            Self::SessionSignals => &SESSION_SIGNAL,
            Self::SessionSkillUses => &SESSION_SKILL,
            Self::SessionTokens => &SESSION_TYPE,
            Self::SessionToolCalls => &SESSION_TOOL,
            Self::Signals => &SIGNAL,
            Self::SkillUses => &SKILL,
            Self::Tokens => &USAGE_TYPE,
            Self::ToolCalls => &TOOL,
        }
    }

    /// Where the family comes from and what its labels mean, for `docs/metrics.md`.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::AccountInfo => "the `account` rows the agents report",
            Self::BuildInfo => "the workspace version",
            Self::Events | Self::ReasoningTokens | Self::Tokens => {
                "`usage_event` rows, all history"
            }
            Self::LastEventAt => "`usage_event` rows, all history, per profile",
            Self::MachineInfo => "the store's machine id, `[machine].name` and the OS",
            Self::ParseIssues => "unresolved `parse_issue` rows",
            Self::QuotaFetchedAt | Self::QuotaResetsAt | Self::QuotaUsedRatio => {
                "the newest successful quota poll of each profile"
            }
            Self::Series | Self::SnapshotTimestamp => "this output",
            Self::SessionEvents
            | Self::SessionInfo
            | Self::SessionLastEventAt
            | Self::SessionReasoningTokens
            | Self::SessionSignals
            | Self::SessionSkillUses
            | Self::SessionStartAt
            | Self::SessionTokens
            | Self::SessionToolCalls => "sessions with an event inside `[metrics].session_window`",
            Self::Signals => "`session_signal` rows, all history",
            Self::SkillUses => "`tool_call` rows that loaded a skill, all history",
            Self::ToolCalls => "`tool_call` rows, all history",
        }
    }
}

/// The catalog as the Markdown table embedded in `docs/metrics.md`.
#[must_use]
pub fn catalog_markdown() -> String {
    let mut out = String::from(
        "| Metric | Type | Labels | Source | Meaning |\n| --- | --- | --- | --- | --- |\n",
    );
    for family in Family::ALL {
        let kind = match family.kind() {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
        };
        let labels = family
            .labels()
            .iter()
            .map(|l| format!("`{l}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "| `{}` | {kind} | {labels} | {} | {} |",
            family.name(),
            family.source(),
            family.help(),
        );
    }
    out.pop();
    out
}

/// Labels a redaction can hash or drop. `cwd` is not among them: no metric carries it.
pub const REDACTABLE: [&str; 6] = [
    "account",
    "client",
    "git_branch",
    "project",
    "session_id",
    "skill",
];

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
