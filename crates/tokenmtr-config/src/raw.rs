//! `RawConfig`: the file as typed data, before cross-field validation.
//!
//! Each struct mirrors a table of `docs/config.md` one to one, and rejects unknown keys. Values
//! that can be checked in isolation (ids, enums, durations, percentages) are parsed here, so the
//! codec reports them with a source span. `None` means "key absent: apply the default".

use std::collections::BTreeMap;

use jiff::SignedDuration;
use jiff::civil::Date;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use secrecy::SecretString;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokenmtr_core::{
    AgentId, BillingPeriod, BudgetId, BudgetMeasure, BudgetPeriod, Currency, ModelId, Plan,
    ProfileId, ProjectKey, ProviderId, SinkId, SubscriptionId, SubscriptionSpec,
};

use crate::model::{
    AccountName, ClientName, Dataset, FileMode, GitName, LogFormat, LogLevel, MenubarRing,
    Partition, Percent, RedactColumn, RedactMode,
};

/// Why a duration string is rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DurationError {
    /// The string is empty.
    #[error("duration must not be empty")]
    Empty,
    /// The string does not start with digits.
    #[error("duration {0:?} must start with a whole number")]
    NoNumber(String),
    /// The number has no unit.
    #[error("duration {0:?} needs a unit: s, m, h or d")]
    MissingUnit(String),
    /// The unit is not `s`, `m`, `h` or `d`.
    #[error("unknown duration unit {unit:?} in {text:?}; use s, m, h or d")]
    UnknownUnit {
        /// The whole text.
        text: String,
        /// The unit part.
        unit: String,
    },
    /// The duration is zero or overflows.
    #[error("duration {0:?} must be positive and fit in 64 bits of seconds")]
    OutOfRange(String),
}

/// Parses `<whole number><unit>` with unit `s`, `m`, `h` or `d` (e.g. `5m`, `1d`). Zero is
/// rejected.
///
/// # Errors
///
/// [`DurationError`] naming the first problem.
pub fn parse_duration(text: &str) -> Result<SignedDuration, DurationError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(DurationError::Empty);
    }
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| DurationError::MissingUnit(text.to_owned()))?;
    if split == 0 {
        return Err(DurationError::NoNumber(text.to_owned()));
    }
    let (digits, unit) = text.split_at(split);
    let seconds_per_unit: i64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => {
            return Err(DurationError::UnknownUnit {
                text: text.to_owned(),
                unit: unit.to_owned(),
            });
        }
    };
    digits
        .parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(seconds_per_unit))
        .filter(|seconds| *seconds > 0)
        .map(SignedDuration::from_secs)
        .ok_or_else(|| DurationError::OutOfRange(text.to_owned()))
}

/// A duration written as a string such as `"5m"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawDuration(pub SignedDuration);

impl<'de> Deserialize<'de> for RawDuration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DurationVisitor;
        impl Visitor<'_> for DurationVisitor {
            type Value = RawDuration;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a duration string such as \"5m\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<RawDuration, E> {
                parse_duration(v).map(RawDuration).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(DurationVisitor)
    }
}

/// A decimal amount written as a TOML integer or float (`300` or `300.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount(pub Decimal);

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AmountVisitor;
        impl Visitor<'_> for AmountVisitor {
            type Value = Amount;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a number")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Amount, E> {
                Decimal::from_f64(v)
                    .map(Amount)
                    .ok_or_else(|| E::custom("number is not finite or out of range"))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Amount, E> {
                Ok(Amount(Decimal::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Amount, E> {
                Ok(Amount(Decimal::from(v)))
            }
        }
        deserializer.deserialize_any(AmountVisitor)
    }
}

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let float = self
            .0
            .to_f64()
            .ok_or_else(|| serde::ser::Error::custom("amount out of range"))?;
        serializer.serialize_f64(float)
    }
}

/// The whole file. An empty file is valid and means "all defaults".
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawConfig {
    pub schema_version: Option<u32>,
    pub account: Option<AccountName>,
    pub machine: RawMachine,
    pub display: RawDisplay,
    pub quota: RawQuota,
    pub fx: RawFx,
    pub menubar: RawMenubar,
    pub alerts: RawAlerts,
    pub advisor: RawAdvisor,
    pub backup: RawBackup,
    pub import: RawImport,
    pub integrations: RawIntegrations,
    pub git: RawGit,
    pub projects: Vec<RawProject>,
    pub budgets: Vec<RawBudget>,
    pub pricing: RawPricing,
    pub subscriptions: Vec<RawSubscription>,
    pub daemon: RawDaemon,
    pub logging: RawLogging,
    pub store: RawStore,
    pub privacy: RawPrivacy,
    pub agents: RawAgents,
    pub sinks: Vec<RawSink>,
    pub metrics: RawMetrics,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawMachine {
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawDisplay {
    pub timezone: Option<String>,
    pub currency: Option<Currency>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawQuota {
    pub enabled: Option<bool>,
    pub interval: Option<RawDuration>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawFx {
    pub enabled: Option<bool>,
    pub fixed_eur_per_usd: Option<Amount>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawMenubar {
    pub agents: Option<Vec<AgentId>>,
    pub rings: Option<Vec<MenubarRing>>,
    pub hidden_profiles: Option<Vec<ProfileId>>,
    pub shortcut: Option<String>,
    pub live_window: Option<RawDuration>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawAlerts {
    pub quota_thresholds: Option<Vec<Percent>>,
    pub quota_reset: Option<bool>,
    pub budget_thresholds: Option<Vec<Percent>>,
    pub sink_failing_after: Option<RawDuration>,
    pub daemon_stale_after: Option<RawDuration>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawAdvisor {
    pub headroom: Option<Percent>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawBackup {
    pub enabled: Option<bool>,
    pub interval: Option<RawDuration>,
    pub keep: Option<u32>,
    pub dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawImport {
    pub claude_stats_cache: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawIntegrations {
    pub superset: RawSuperset,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawSuperset {
    pub enabled: Option<bool>,
    pub dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawGit {
    pub default_remote: Option<GitName>,
    pub default_branch: Option<GitName>,
}

/// One `[[projects]]` rule; also the argument of
/// [`ConfigDocument::add_project_rule`](crate::ConfigDocument::add_project_rule).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawProject {
    /// The glob, `~` allowed.
    #[serde(rename = "match")]
    pub pattern: String,
    pub name: ProjectKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientName>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBudget {
    pub id: BudgetId,
    pub period: BudgetPeriod,
    pub amount: Amount,
    pub currency: Option<Currency>,
    pub measure: Option<BudgetMeasure>,
    pub agents: Option<Vec<AgentId>>,
    pub profiles: Option<Vec<ProfileId>>,
    pub projects: Option<Vec<ProjectKey>>,
    pub clients: Option<Vec<ClientName>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawPricing {
    pub auto_update: Option<bool>,
    pub overrides: Vec<RawOverride>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawOverride {
    pub model: ModelId,
    /// Restricts the override to events served by this provider (`Override::for_provider`).
    pub provider: Option<ProviderId>,
    pub input_per_m: Amount,
    pub output_per_m: Amount,
    pub cache_read_per_m: Option<Amount>,
    pub cache_write_5m_per_m: Option<Amount>,
    pub cache_write_1h_per_m: Option<Amount>,
}

/// One `[[subscriptions]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawSubscription {
    pub id: SubscriptionId,
    pub provider: ProviderId,
    pub plan: Plan,
    pub agents: Vec<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<ProfileId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee: Option<Amount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_currency: Option<Currency>,
    pub period: BillingPeriod,
    pub start: Date,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Date>,
}

impl From<&SubscriptionSpec> for RawSubscription {
    /// The entry that parses back into `spec`, with `fee` and `fee_currency` explicit.
    fn from(spec: &SubscriptionSpec) -> Self {
        Self {
            id: spec.id.clone(),
            provider: spec.provider.clone(),
            plan: spec.plan,
            agents: spec.agents.clone(),
            profiles: spec.profiles.clone(),
            fee: Some(Amount(Decimal::new(spec.fee.micros(), 6).normalize())),
            fee_currency: Some(spec.fee.currency()),
            period: spec.period,
            start: spec.start,
            end: spec.end,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawDaemon {
    pub scan_interval: Option<RawDuration>,
    pub watch: Option<bool>,
    pub log_level: Option<LogLevel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawLogging {
    pub format: Option<LogFormat>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawStore {
    pub path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawPrivacy {
    pub redact_paths: Option<bool>,
}

/// `[agents.*]`: one field per known agent, so an unknown agent is an unknown key.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawAgents {
    pub claude: RawAgent<RawClaudeProfile>,
    pub pi: RawAgent<RawPiProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(
    deny_unknown_fields,
    default,
    bound(deserialize = "P: Deserialize<'de>")
)]
pub struct RawAgent<P> {
    pub enabled: Option<bool>,
    pub account: Option<AccountName>,
    pub profiles: Vec<P>,
}

impl<P> Default for RawAgent<P> {
    fn default() -> Self {
        Self {
            enabled: None,
            account: None,
            profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawClaudeProfile {
    pub id: ProfileId,
    pub roots: Option<Vec<String>>,
    pub config_dir: Option<String>,
    pub account: Option<AccountName>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPiProfile {
    pub id: ProfileId,
    pub roots: Option<Vec<String>>,
    pub account: Option<AccountName>,
}

/// The `kind` of a sink. `csv` and `jsonl` both map to a file sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawSinkKind {
    Csv,
    Jsonl,
    Sqlite,
}

/// One `[[sinks]]` entry. Kind-specific keys are checked against `kind` during validation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSink {
    pub id: SinkId,
    pub kind: RawSinkKind,
    pub interval: Option<RawDuration>,
    pub datasets: Option<Vec<Dataset>>,
    pub redact: Option<BTreeMap<RedactColumn, RedactMode>>,
    pub dir: Option<String>,
    pub path: Option<String>,
    pub mode: Option<FileMode>,
    pub partition: Option<Partition>,
}

/// `[metrics]`: the Prometheus outputs and what they share.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawMetrics {
    pub refresh_interval: Option<RawDuration>,
    pub session_window: Option<RawDuration>,
    pub exporters: Vec<RawExporter>,
    pub remote_write: Vec<RawRemoteWrite>,
}

/// One `[[metrics.exporters]]` entry: a scrape endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawExporter {
    pub id: SinkId,
    pub listen: Option<String>,
    pub exclude_projects: Option<Vec<String>>,
    pub exclude_paths: Option<Vec<String>>,
    pub redact: Option<BTreeMap<RedactColumn, RedactMode>>,
    pub auth: Option<RawAuth>,
}

/// One `[[metrics.remote_write]]` entry: a push target.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRemoteWrite {
    pub id: SinkId,
    pub url: String,
    pub interval: Option<RawDuration>,
    pub exclude_projects: Option<Vec<String>>,
    pub exclude_paths: Option<Vec<String>>,
    pub redact: Option<BTreeMap<RedactColumn, RedactMode>>,
    pub tenant: Option<String>,
    pub auth: Option<RawAuth>,
    pub external_labels: Option<BTreeMap<String, String>>,
}

/// The `kind` of an output's `auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawAuthKind {
    Basic,
    Bearer,
}

/// An output's `auth` table. Kind-specific keys are checked against `kind` during validation,
/// as for [`RawSink`]. Secret values are [`SecretString`]s, so `Debug` never shows them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAuth {
    pub kind: RawAuthKind,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub password_env: Option<String>,
    pub password_file: Option<String>,
    pub token: Option<SecretString>,
    pub token_env: Option<String>,
    pub token_file: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_units() {
        assert_eq!(parse_duration("5m"), Ok(SignedDuration::from_mins(5)));
        assert_eq!(parse_duration("30s"), Ok(SignedDuration::from_secs(30)));
        assert_eq!(parse_duration("2h"), Ok(SignedDuration::from_hours(2)));
        assert_eq!(parse_duration("1d"), Ok(SignedDuration::from_hours(24)));
    }

    #[test]
    fn durations_reject_bad_input() {
        assert_eq!(parse_duration(""), Err(DurationError::Empty));
        assert!(matches!(
            parse_duration("m"),
            Err(DurationError::NoNumber(_))
        ));
        assert!(matches!(
            parse_duration("5"),
            Err(DurationError::MissingUnit(_))
        ));
        assert!(matches!(
            parse_duration("5w"),
            Err(DurationError::UnknownUnit { .. })
        ));
        assert!(matches!(
            parse_duration("0m"),
            Err(DurationError::OutOfRange(_))
        ));
        assert!(matches!(
            parse_duration("-5m"),
            Err(DurationError::NoNumber(_))
        ));
        assert!(matches!(
            parse_duration("99999999999999999999s"),
            Err(DurationError::OutOfRange(_))
        ));
    }
}
