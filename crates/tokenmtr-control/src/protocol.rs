//! Wire types of control protocol v1 (D24, D52).
//!
//! Every message is one JSON object on one line. The `type` field names the variant. Requests go
//! client → daemon; each gets exactly one [`Response`], except [`Request::Subscribe`], which is
//! answered with [`Response::Subscribed`] and then followed by [`Response::Event`] lines for as
//! long as the connection lives.

use camino::Utf8PathBuf;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokenmtr_core::{AgentId, Alert, SinkId};

/// The protocol version this build speaks. Bumped on any change a v1 peer would misread; a
/// daemon answers a [`Request::Hello`] with another version with [`Response::Incompatible`].
pub const PROTOCOL_VERSION: u32 = 1;

/// A dataset whose rows changed. Mirrors the store's datasets without depending on its types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dataset {
    /// `usage_events`.
    UsageEvents,
    /// `tool_calls`.
    ToolCalls,
    /// `subscriptions`.
    Subscriptions,
    /// `quota_snapshots`.
    QuotaSnapshots,
    /// `fx_rates`.
    FxRates,
}

impl Dataset {
    /// Every dataset.
    pub const ALL: [Self; 5] = [
        Self::UsageEvents,
        Self::ToolCalls,
        Self::Subscriptions,
        Self::QuotaSnapshots,
        Self::FxRates,
    ];
}

/// Who is connecting; sent in [`Request::Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Program name, e.g. `tokenmtr-app`.
    pub name: String,
    /// Its version.
    pub version: String,
    /// The store schema version the client was built for, if it reads the store.
    pub schema: Option<u32>,
}

/// A client request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// The first message of every connection.
    Hello {
        /// The protocol version the client speaks.
        protocol: u32,
        /// Who the client is.
        client: ClientInfo,
    },
    /// Daemon health and task state.
    Status,
    /// Scan agent logs now, all agents or one.
    ScanNow {
        /// Restrict the scan to this agent.
        agent: Option<AgentId>,
    },
    /// Export to sinks now, all sinks or one.
    ExportNow {
        /// Restrict the export to this sink.
        sink: Option<SinkId>,
    },
    /// Write a backup now.
    BackupNow,
    /// Poll Claude quota now.
    RefreshQuota,
    /// Re-read the config file now.
    Reload,
    /// Open problems: parse issues and the last config error.
    Issues,
    /// Start receiving [`Event`]s on this connection.
    Subscribe,
    /// Stop the daemon gracefully, as SIGTERM does (D56). Answered before the daemon stops.
    Shutdown,
}

/// What the daemon pushes to subscribers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The head revision of these datasets moved. Coalesced to at most one per second.
    StoreChanged {
        /// Datasets with new rows, ascending.
        datasets: Vec<Dataset>,
    },
    /// An alert was written for the first time (D36).
    AlertFired {
        /// The alert.
        alert: Alert,
    },
}

/// A daemon answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The handshake succeeded.
    Welcome {
        /// The daemon's protocol version (equal to the client's).
        protocol: u32,
        /// The daemon's package version.
        daemon_version: String,
        /// The store schema version the daemon runs.
        store_schema: u32,
    },
    /// The protocol versions differ. The daemon sends no data and closes the connection.
    Incompatible {
        /// The version the daemon speaks.
        daemon_protocol: u32,
        /// The daemon's package version.
        daemon_version: String,
        /// The store schema version the daemon runs.
        store_schema: u32,
        /// Human-readable cause.
        reason: String,
    },
    /// Answer to [`Request::Status`].
    Status(DaemonStatus),
    /// Answer to [`Request::ScanNow`].
    Scanned(ScanSummary),
    /// Answer to [`Request::ExportNow`].
    Exported(ExportSummary),
    /// Answer to [`Request::BackupNow`].
    BackedUp(BackupSummary),
    /// Answer to [`Request::RefreshQuota`].
    QuotaRefreshed(QuotaSummary),
    /// Answer to [`Request::Reload`]: the new config is active.
    Reloaded,
    /// Answer to [`Request::Issues`].
    Issues(IssueList),
    /// Answer to [`Request::Shutdown`]: the daemon is stopping; the socket disappears when done.
    ShuttingDown,
    /// Answer to [`Request::Subscribe`]; events follow.
    Subscribed,
    /// A pushed event; only after [`Response::Subscribed`].
    Event {
        /// The event.
        event: Event,
    },
    /// The request failed.
    Error {
        /// Machine-readable class.
        kind: ErrorKind,
        /// Human-readable cause; never contains credentials (D55).
        message: String,
    },
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The request is not valid for the current state (no handshake, unknown sink, ...).
    BadRequest,
    /// The feature is switched off in the config.
    Disabled,
    /// The new config is invalid; the old one stays active.
    InvalidConfig,
    /// The operation ran and failed.
    Failed,
    /// The daemon is shutting down.
    ShuttingDown,
}

/// An optional background task; each runs only when its config flag is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    /// Watches agent directories for changes (`daemon.watch`).
    Watch,
    /// Polls Claude quota (`quota.enabled`).
    Quota,
    /// Refreshes ECB rates (`fx.enabled`).
    Fx,
    /// Refreshes the price list (`pricing.auto_update`).
    Pricing,
    /// Writes daily backups (`backup.enabled`).
    Backup,
}

/// Daemon health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// Process id.
    pub pid: u32,
    /// Package version.
    pub daemon_version: String,
    /// Store schema version.
    pub store_schema: u32,
    /// When the daemon started.
    pub started_at: Timestamp,
    /// The last scan that finished, if any.
    pub last_scan_at: Option<Timestamp>,
    /// The error of the last rejected config reload, until a reload succeeds.
    pub config_error: Option<String>,
    /// Optional background tasks the config switched on, ascending.
    pub tasks: Vec<Task>,
    /// Connections currently subscribed to events.
    pub subscribers: u32,
    /// One entry per configured sink.
    pub sinks: Vec<SinkSummary>,
}

/// A sink's last known outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkSummary {
    /// The sink.
    pub id: SinkId,
    /// Start of the current failure streak, `None` while the sink delivers.
    pub failing_since: Option<Timestamp>,
    /// The latest error, while failing.
    pub last_error: Option<String>,
    /// When the sink last delivered or confirmed it is caught up.
    pub last_ok_at: Option<Timestamp>,
}

/// Result of a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanSummary {
    /// One entry per scanned agent.
    pub agents: Vec<AgentScanSummary>,
    /// Wall time in milliseconds.
    pub elapsed_ms: u64,
}

/// One agent's scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentScanSummary {
    /// The agent.
    pub agent: AgentId,
    /// Files parsed.
    pub files_parsed: u64,
    /// Files that failed to parse.
    pub files_failed: u64,
    /// Events inserted.
    pub events_inserted: u64,
    /// Events whose values changed.
    pub events_updated: u64,
}

/// Result of an export run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSummary {
    /// One entry per sink that ran.
    pub sinks: Vec<SinkExportSummary>,
}

/// One sink's run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkExportSummary {
    /// The sink.
    pub sink: SinkId,
    /// Rows delivered.
    pub rows: u64,
    /// Why the run stopped early.
    pub error: Option<String>,
}

/// Result of a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSummary {
    /// The backup file.
    pub path: Utf8PathBuf,
    /// Its size.
    pub size_bytes: u64,
}

/// Result of a quota refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaSummary {
    /// One entry per polled profile.
    pub profiles: Vec<QuotaProfileSummary>,
}

/// One profile's poll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaProfileSummary {
    /// The agent.
    pub agent: AgentId,
    /// The profile id.
    pub profile: String,
    /// `ok`, `signed_out`, `token_expired`, `malformed`, `network_down` or `failed`.
    pub status: String,
}

/// Open problems.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueList {
    /// Unresolved parse issues, oldest first.
    pub parse_issues: Vec<IssueSummary>,
    /// The error of the last rejected config reload.
    pub config_error: Option<String>,
}

/// One unresolved parse issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSummary {
    /// The agent whose logs were read.
    pub agent: AgentId,
    /// Issue class in snake case, e.g. `format_drift`.
    pub kind: String,
    /// What it is about.
    pub subject: String,
    /// Occurrences since it opened.
    pub occurrences: u64,
    /// When it was last seen.
    pub last_seen: Timestamp,
}
