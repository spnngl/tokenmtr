//! The validated configuration: every value here is in a state the daemon can act on.
//!
//! Build it with [`Config::from_raw`] or the `load*` functions; fields are public for reading.
//! Optional paths mean "use the platform default" (D21), which `tokenmtr-platform` resolves.

use std::collections::BTreeMap;
use std::fmt;

use camino::{Utf8Path, Utf8PathBuf};
use globset::GlobMatcher;
use jiff::SignedDuration;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use tokenmtr_core::{
    AgentId, AlertConfig, Budget, Currency, EurPerUsd, Override, ProfileId, ProjectKey, SinkId,
    Subscription,
};

/// The only schema version this crate reads (`schema_version` in the file).
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// A percentage between 1 and 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "u8")]
pub struct Percent(u8);

impl Percent {
    /// The value, 1..=100.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// `advisor.headroom` when unset (D35).
pub(crate) const DEFAULT_HEADROOM: Percent = Percent(80);

impl TryFrom<u8> for Percent {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, String> {
        if (1..=100).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!("percent must be between 1 and 100, got {value}"))
        }
    }
}

/// A client label set by `[[projects]]` rules and used by reports and budgets (non-blank).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClientName(String);

impl ClientName {
    /// The label as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ClientName {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if value.trim().is_empty() {
            Err("client name must not be blank".to_owned())
        } else {
            Ok(Self(value))
        }
    }
}

impl From<ClientName> for String {
    fn from(value: ClientName) -> Self {
        value.0
    }
}

impl fmt::Display for ClientName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A table a sink exports (glossary: dataset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dataset {
    /// `usage_events`.
    UsageEvents,
    /// `tool_calls`.
    ToolCalls,
    /// `quota_snapshots`.
    QuotaSnapshots,
    /// `fx_rates`.
    FxRates,
    /// `subscriptions`.
    Subscriptions,
}

impl Dataset {
    /// Every dataset, the default for a sink (D17).
    pub const ALL: [Self; 5] = [
        Self::UsageEvents,
        Self::ToolCalls,
        Self::QuotaSnapshots,
        Self::FxRates,
        Self::Subscriptions,
    ];

    /// The dataset name as written in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsageEvents => "usage_events",
            Self::ToolCalls => "tool_calls",
            Self::QuotaSnapshots => "quota_snapshots",
            Self::FxRates => "fx_rates",
            Self::Subscriptions => "subscriptions",
        }
    }
}

impl fmt::Display for Dataset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A column a sink may redact (D40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactColumn {
    /// Working directory.
    Cwd,
    /// Project key.
    Project,
    /// Client label.
    Client,
    /// Git branch.
    GitBranch,
    /// Account email or organization.
    Account,
    /// Agent session id.
    SessionId,
}

/// What redaction does to a column (D40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactMode {
    /// Keyed blake3, 16 hex chars.
    Hash,
    /// Empty value.
    Drop,
}

/// File format of a file sink (D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileFormat {
    /// Comma-separated values.
    Csv,
    /// One JSON object per line.
    Jsonl,
}

/// How a file sink writes rows (D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// Append changed rows with their revision.
    #[default]
    Append,
    /// Rewrite touched partitions with one row per id.
    Snapshot,
}

/// How a file sink splits files by row date (D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Partition {
    /// One file per month.
    #[default]
    Month,
    /// One file per day.
    Day,
    /// A single file.
    All,
}

/// The `kind`-specific part of a sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkTarget {
    /// CSV or JSONL files in a directory.
    File(FileSink),
    /// A SQLite database file.
    Sqlite(SqliteSink),
}

/// Settings of a CSV or JSONL sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSink {
    /// File format.
    pub format: FileFormat,
    /// Output directory; `None` is `<data_dir>/exports` (D21).
    pub dir: Option<Utf8PathBuf>,
    /// Append or snapshot.
    pub mode: FileMode,
    /// Partitioning by row date.
    pub partition: Partition,
}

/// Settings of a SQLite sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteSink {
    /// Database file; `None` is `<data_dir>/exports/export.db` (D21).
    pub path: Option<Utf8PathBuf>,
}

/// One `[[sinks]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkConfig {
    /// Unique id.
    pub id: SinkId,
    /// Minimum time between export runs (D47).
    pub interval: SignedDuration,
    /// Exported datasets: non-empty, unique.
    pub datasets: Vec<Dataset>,
    /// Per-column redaction (D40).
    pub redact: BTreeMap<RedactColumn, RedactMode>,
    /// Kind-specific settings.
    pub target: SinkTarget,
}

/// Where an agent profile reads from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileLocation {
    /// Platform and environment discovery.
    Default,
    /// Explicit session roots (absolute).
    Roots(Vec<Utf8PathBuf>),
    /// A config dir holding roots, credentials and the stats cache (Claude only).
    ConfigDir(Utf8PathBuf),
}

/// A named group of an agent's roots with its own credentials (D30).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileConfig {
    /// Profile id, unique within its agent.
    pub id: ProfileId,
    /// Where it reads from.
    pub location: ProfileLocation,
}

/// One agent's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    /// Whether the agent is collected.
    pub enabled: bool,
    /// Declared profiles; always contains `default`, first when it was implicit.
    pub profiles: Vec<ProfileConfig>,
}

/// `[machine]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineConfig {
    /// Display label; `None` is the host name (D30).
    pub name: Option<String>,
}

/// The time zone that buckets days (D8).
#[derive(Debug, Clone)]
pub enum TimeZoneSetting {
    /// The system zone.
    System,
    /// An IANA zone, resolved against the tz database.
    Named(TimeZone),
}

/// `[display]`.
#[derive(Debug, Clone)]
pub struct DisplayConfig {
    /// Day bucketing zone.
    pub timezone: TimeZoneSetting,
    /// Currency shown.
    pub currency: Currency,
}

/// `[quota]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaConfig {
    /// Whether the daemon polls quota (D26).
    pub enabled: bool,
    /// Poll interval, at least one minute.
    pub interval: SignedDuration,
}

/// `[fx]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxConfig {
    /// Whether the daemon fetches ECB rates (D27).
    pub enabled: bool,
    /// Rate that overrides the ECB entirely.
    pub fixed_eur_per_usd: Option<EurPerUsd>,
}

/// What the tray icon title shows (D29).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenubarTitle {
    /// Money paid today.
    #[default]
    PaidToday,
    /// API value today.
    ApiToday,
    /// The quota window closest to its limit.
    TightestQuota,
    /// No title.
    #[serde(rename = "none")]
    Hidden,
}

/// `[menubar]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenubarConfig {
    /// Followed agents, one recap tab each: non-empty, enabled, unique.
    pub agents: Vec<AgentId>,
    /// Tray title.
    pub title: MenubarTitle,
    /// Global shortcut; `None` disables (D44). Parsed by the desktop shell.
    pub shortcut: Option<String>,
    /// Window of the Live block (D58).
    pub live_window: SignedDuration,
}

/// `[alerts]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertSettings {
    /// Rule inputs for `tokenmtr_core::evaluate_alerts`: thresholds sorted and unique.
    pub rules: AlertConfig,
    /// Daemon heartbeat age after which the app raises `daemon_stale` (D36).
    pub daemon_stale_after: SignedDuration,
}

/// `[advisor]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvisorConfig {
    /// Maximum scaled utilization a recommended plan may reach (D35).
    pub headroom: Percent,
}

/// `[backup]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    /// Whether the daemon backs up daily (D37).
    pub enabled: bool,
    /// Time between backups.
    pub interval: SignedDuration,
    /// Backups kept, at least 1.
    pub keep: u32,
    /// Backup directory; `None` is `<data_dir>/backups`.
    pub dir: Option<Utf8PathBuf>,
}

/// `[import]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportConfig {
    /// Import Claude's stats cache on each scan (D38).
    pub claude_stats_cache: bool,
}

/// One `[[projects]]` rule (D31): the first rule whose glob matches a cwd names its project.
#[derive(Debug, Clone)]
pub struct ProjectRule {
    pub(crate) matcher: GlobMatcher,
    /// Project name given to matching cwds.
    pub name: ProjectKey,
    /// Optional client label.
    pub client: Option<ClientName>,
}

impl ProjectRule {
    /// The glob with `~` expanded.
    #[must_use]
    pub fn pattern(&self) -> &str {
        self.matcher.glob().glob()
    }

    /// Whether `cwd` matches the rule. `*` does not cross `/`; `**` does.
    #[must_use]
    pub fn matches(&self, cwd: &Utf8Path) -> bool {
        self.matcher.is_match(cwd.as_std_path())
    }
}

/// Which usage a budget counts; every `Some` list is non-empty and unique.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BudgetScope {
    /// Enabled agents.
    pub agents: Option<Vec<AgentId>>,
    /// Declared profiles.
    pub profiles: Option<Vec<ProfileId>>,
    /// Project names or keys.
    pub projects: Option<Vec<ProjectKey>>,
    /// Client labels.
    pub clients: Option<Vec<ClientName>>,
}

/// One `[[budgets]]` entry (D34): the core budget plus its filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetEntry {
    /// Limit, period and measure.
    pub budget: Budget,
    /// What it counts; `None` fields count everything.
    pub scope: BudgetScope,
}

/// `[pricing]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingConfig {
    /// Whether the daemon refreshes the `LiteLLM` snapshot daily (D11).
    pub auto_update: bool,
    /// Prices that beat `LiteLLM`.
    pub overrides: Vec<Override>,
}

/// Verbosity of the daemon log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Normal operation.
    #[default]
    Info,
    /// Debugging detail.
    Debug,
    /// Everything.
    Trace,
}

/// Line format of the daemon log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// Human-readable lines.
    #[default]
    Text,
    /// One JSON object per line.
    Json,
}

/// `[logging]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggingConfig {
    /// Line format of the log file and of the terminal output.
    pub format: LogFormat,
}

/// `[daemon]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonConfig {
    /// Safety-net full sweep interval (D16).
    pub scan_interval: SignedDuration,
    /// Whether file system events trigger scans.
    pub watch: bool,
    /// Log verbosity.
    pub log_level: LogLevel,
}

/// `[store]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    /// Store file; `None` is `<data_dir>/usage.db` (D21).
    pub path: Option<Utf8PathBuf>,
}

/// `[privacy]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyConfig {
    /// Hash paths at ingest (D13).
    pub redact_paths: bool,
}

/// The whole configuration. See `docs/config.md` for every key and default.
#[derive(Debug, Clone)]
pub struct Config {
    /// `[machine]`.
    pub machine: MachineConfig,
    /// `[display]`.
    pub display: DisplayConfig,
    /// `[quota]`.
    pub quota: QuotaConfig,
    /// `[fx]`.
    pub fx: FxConfig,
    /// `[menubar]`.
    pub menubar: MenubarConfig,
    /// `[alerts]`.
    pub alerts: AlertSettings,
    /// `[advisor]`.
    pub advisor: AdvisorConfig,
    /// `[backup]`.
    pub backup: BackupConfig,
    /// `[import]`.
    pub import: ImportConfig,
    /// `[[projects]]`, in match order.
    pub projects: Vec<ProjectRule>,
    /// `[[budgets]]`.
    pub budgets: Vec<BudgetEntry>,
    /// `[pricing]`.
    pub pricing: PricingConfig,
    /// `[[subscriptions]]`: ids unique, none overlapping (D28).
    pub subscriptions: Vec<Subscription>,
    /// `[daemon]`.
    pub daemon: DaemonConfig,
    /// `[logging]`.
    pub logging: LoggingConfig,
    /// `[store]`.
    pub store: StoreConfig,
    /// `[privacy]`.
    pub privacy: PrivacyConfig,
    /// `[agents.*]`: one entry per known agent.
    pub agents: BTreeMap<AgentId, AgentConfig>,
    /// `[[sinks]]`: ids unique.
    pub sinks: Vec<SinkConfig>,
}

impl Config {
    /// The agents that are collected.
    pub fn enabled_agents(&self) -> impl Iterator<Item = AgentId> + '_ {
        self.agents
            .iter()
            .filter(|(_, agent)| agent.enabled)
            .map(|(id, _)| *id)
    }
}
