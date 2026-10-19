//! The validated configuration: every value here is in a state the daemon can act on.
//!
//! Build it with [`Config::from_raw`] or the `load*` functions; fields are public for reading.
//! Optional paths mean "use the platform default", which `tokenmtr-platform` resolves.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;

use camino::{Utf8Path, Utf8PathBuf};
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use jiff::SignedDuration;
use jiff::tz::TimeZone;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
pub use tokenmtr_core::Dataset;
use tokenmtr_core::{
    AgentId, AlertConfig, Budget, Currency, EurPerUsd, Override, ProfileId, ProjectKey, SinkId,
    Subscription,
};
use url::Url;

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

/// `advisor.headroom` when unset.
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

/// A git remote or branch name, such as `origin` or `main`: non-blank, without whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct GitName(String);

impl GitName {
    /// `origin`, the remote `git clone` creates.
    #[must_use]
    pub fn origin() -> Self {
        Self("origin".to_owned())
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GitName {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if value.is_empty() || value.contains(char::is_whitespace) {
            Err("git name must be non-blank and contain no whitespace".to_owned())
        } else {
            Ok(Self(value))
        }
    }
}

impl From<GitName> for String {
    fn from(value: GitName) -> Self {
        value.0
    }
}

impl fmt::Display for GitName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A column a sink may redact.
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

/// What redaction does to a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactMode {
    /// Keyed blake3, 16 hex chars.
    Hash,
    /// Empty value.
    Drop,
}

/// File format of a file sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileFormat {
    /// Comma-separated values.
    Csv,
    /// One JSON object per line.
    Jsonl,
}

/// How a file sink writes rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// Append changed rows with their revision.
    #[default]
    Append,
    /// Rewrite touched partitions with one row per id.
    Snapshot,
}

/// How a file sink splits files by row date.
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
    /// Output directory; `None` is `<data_dir>/exports`.
    pub dir: Option<Utf8PathBuf>,
    /// Append or snapshot.
    pub mode: FileMode,
    /// Partitioning by row date.
    pub partition: Partition,
}

/// Settings of a SQLite sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteSink {
    /// Database file; `None` is `<data_dir>/exports/export.db`.
    pub path: Option<Utf8PathBuf>,
}

/// One `[[sinks]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkConfig {
    /// Unique id.
    pub id: SinkId,
    /// Minimum time between export runs.
    pub interval: SignedDuration,
    /// Exported datasets: non-empty, unique.
    pub datasets: Vec<Dataset>,
    /// Per-column redaction.
    pub redact: BTreeMap<RedactColumn, RedactMode>,
    /// Kind-specific settings.
    pub target: SinkTarget,
}

/// Where an output's secret comes from. Env and file sources are read when the output starts,
/// never at config load.
#[derive(Debug, Clone)]
pub enum SecretSource {
    /// The value is in the config file.
    Inline(SecretString),
    /// The name of an environment variable of the daemon.
    Env(String),
    /// A file holding the secret; absolute, private to the user.
    File(Utf8PathBuf),
}

impl PartialEq for SecretSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Inline(a), Self::Inline(b)) => a.expose_secret() == b.expose_secret(),
            (Self::Env(a), Self::Env(b)) => a == b,
            (Self::File(a), Self::File(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for SecretSource {}

/// How an output authenticates its peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// HTTP basic.
    Basic {
        /// The user name; never contains `:`.
        username: String,
        /// The password.
        password: SecretSource,
    },
    /// A bearer token.
    Bearer {
        /// The token.
        token: SecretSource,
    },
}

/// Compiles a glob where `*` does not cross `/`: the dialect of `[[projects]] match` and of the
/// metrics exclusions.
///
/// # Errors
///
/// The glob crate's rejection of `pattern`.
pub(crate) fn path_glob(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

/// Glob patterns that compile to one matcher. Equality compares the patterns.
#[derive(Debug, Clone)]
pub struct GlobList {
    patterns: Vec<String>,
    set: GlobSet,
}

impl GlobList {
    /// Compiles `patterns` (already `~`-expanded) with `/` kept literal, as `[[projects]]` does.
    ///
    /// # Errors
    ///
    /// The first pattern the glob crate rejects.
    pub fn new(patterns: Vec<String>) -> Result<Self, globset::Error> {
        let mut set = GlobSetBuilder::new();
        for pattern in &patterns {
            set.add(path_glob(pattern)?);
        }
        Ok(Self {
            set: set.build()?,
            patterns,
        })
    }

    /// Whether `text` matches any pattern.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        self.set.is_match(text)
    }

    /// Whether there are no patterns.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// The patterns as written (after `~` expansion).
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

impl Default for GlobList {
    fn default() -> Self {
        Self {
            patterns: Vec::new(),
            set: GlobSet::empty(),
        }
    }
}

impl PartialEq for GlobList {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for GlobList {}

/// What one metrics output leaves out and hides. Exclusion drops rows from this output only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputFilter {
    /// Globs on the project key (`host/owner/repo`), its display name and its git remote. `*` does
    /// not cross `/`.
    pub exclude_projects: GlobList,
    /// Globs on the working directory; never set with `privacy.redact_paths`.
    pub exclude_paths: GlobList,
    /// Per-label redaction; never has `cwd`.
    pub redact: BTreeMap<RedactColumn, RedactMode>,
}

/// One `[[metrics.exporters]]` entry: a scrape endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExporterConfig {
    /// Unique id, shared with sinks and remote-write outputs.
    pub id: SinkId,
    /// Address to bind; unique among exporters.
    pub listen: SocketAddr,
    /// Required credentials, if any.
    pub auth: Option<Auth>,
    /// Exclusion and redaction.
    pub filter: OutputFilter,
}

/// The labels a remote-write output adds to every series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalLabels {
    /// The default: `machine` is the machine name.
    Machine,
    /// Exactly these; names are valid and not catalog labels.
    Explicit(BTreeMap<String, String>),
}

/// One `[[metrics.remote_write]]` entry: a push target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWriteConfig {
    /// Unique id, shared with sinks and exporters.
    pub id: SinkId,
    /// The receiver endpoint: https, or http to this machine; no credentials.
    pub url: Url,
    /// Time between pushes.
    pub interval: SignedDuration,
    /// Exclusion and redaction.
    pub filter: OutputFilter,
    /// `X-Scope-OrgID`, when set.
    pub tenant: Option<String>,
    /// Credentials, if any.
    pub auth: Option<Auth>,
    /// Labels added to every series.
    pub external_labels: ExternalLabels,
}

/// `[metrics]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsConfig {
    /// Minimum time between snapshot recomputes.
    pub refresh_interval: SignedDuration,
    /// A session family keeps sessions with a usage event this recent.
    pub session_window: SignedDuration,
    /// `[[metrics.exporters]]`.
    pub exporters: Vec<ExporterConfig>,
    /// `[[metrics.remote_write]]`.
    pub remote_write: Vec<RemoteWriteConfig>,
}

/// The labels the metrics carry. An `external_labels` key cannot be one of them.
pub const METRIC_LABELS: &[&str] = &[
    "account",
    "agent",
    "billing",
    "client",
    "error",
    "git_branch",
    "kind",
    "mcp_server",
    "model",
    "profile",
    "project",
    "provider",
    "session_id",
    "tool",
    "type",
    "version",
    "window",
];

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

/// A named group of an agent's roots with its own credentials.
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
    /// Display label; `None` is the host name.
    pub name: Option<String>,
}

/// The time zone that buckets days.
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
    /// Whether the daemon polls quota.
    pub enabled: bool,
    /// Poll interval, at least one minute.
    pub interval: SignedDuration,
}

/// `[fx]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxConfig {
    /// Whether the daemon fetches ECB rates.
    pub enabled: bool,
    /// Rate that overrides the ECB entirely.
    pub fixed_eur_per_usd: Option<EurPerUsd>,
}

/// A kind of quota window the menu bar draws as a ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenubarRing {
    /// The five-hour session window (`five_hour`).
    Session,
    /// The seven-day window across models (`seven_day`).
    Weekly,
    /// Every weekly window scoped to one model (`weekly_scoped:<model>`).
    PerModel,
}

impl MenubarRing {
    /// Every kind, in drawing order.
    pub const ALL: [Self; 3] = [Self::Session, Self::Weekly, Self::PerModel];

    /// The config spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
            Self::PerModel => "per_model",
        }
    }
}

impl fmt::Display for MenubarRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `[menubar]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenubarConfig {
    /// Followed agents, one recap tab each: non-empty, enabled, unique.
    pub agents: Vec<AgentId>,
    /// Window kinds drawn as rings in the menu bar, unique; empty shows the plain icon.
    pub rings: Vec<MenubarRing>,
    /// Profiles whose quota gets no rings, unique.
    pub hidden_profiles: Vec<ProfileId>,
    /// Global shortcut; `None` disables. Parsed by the desktop shell.
    pub shortcut: Option<String>,
    /// Window of the Live block.
    pub live_window: SignedDuration,
}

/// `[alerts]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertSettings {
    /// Rule inputs for `tokenmtr_core::evaluate_alerts`: thresholds sorted and unique.
    pub rules: AlertConfig,
    /// Daemon heartbeat age after which the app raises `daemon_stale`.
    pub daemon_stale_after: SignedDuration,
}

/// `[advisor]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvisorConfig {
    /// Maximum scaled utilization a recommended plan may reach.
    pub headroom: Percent,
}

/// `[backup]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    /// Whether the daemon backs up daily.
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
    /// Import Claude's stats cache on each scan.
    pub claude_stats_cache: bool,
}

/// `[integrations]`: other apps' data, read-only, for hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationsConfig {
    /// `[integrations.superset]`.
    pub superset: SupersetConfig,
}

/// `[integrations.superset]`: Superset's database names the repository and branch of a
/// worktree that no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersetConfig {
    /// Whether Superset's database is read.
    pub enabled: bool,
    /// Superset's data directory; `None` is `~/.superset`.
    pub dir: Option<Utf8PathBuf>,
}

/// `[git]`: how a repository becomes a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitConfig {
    /// The remote whose URL is the project key; `origin`, then the first remote by name, stand
    /// in when a repository lacks it.
    pub default_remote: GitName,
    /// The work branch, reserved for branch statistics; nothing reads it yet.
    pub default_branch: Option<GitName>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            default_remote: GitName::origin(),
            default_branch: None,
        }
    }
}

/// One `[[projects]]` rule: the first rule whose glob matches a cwd names its project.
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

/// One `[[budgets]]` entry: the core budget plus its filters.
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
    /// Whether the daemon refreshes the `LiteLLM` snapshot daily.
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
    /// Safety-net full sweep interval.
    pub scan_interval: SignedDuration,
    /// Whether file system events trigger scans.
    pub watch: bool,
    /// Log verbosity.
    pub log_level: LogLevel,
}

/// `[store]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    /// Store file; `None` is `<data_dir>/usage.db`.
    pub path: Option<Utf8PathBuf>,
}

/// `[privacy]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyConfig {
    /// Hash paths at ingest.
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
    /// `[integrations]`.
    pub integrations: IntegrationsConfig,
    /// `[git]`.
    pub git: GitConfig,
    /// `[[projects]]`, in match order.
    pub projects: Vec<ProjectRule>,
    /// `[[budgets]]`.
    pub budgets: Vec<BudgetEntry>,
    /// `[pricing]`.
    pub pricing: PricingConfig,
    /// `[[subscriptions]]`: ids unique, none overlapping.
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
    /// `[metrics]`.
    pub metrics: MetricsConfig,
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
