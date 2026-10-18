//! Operational state: sinks (D41), daemon, ingestion, parse issues, alerts, backups, FX rates.

use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::Serialize;
use tokenmtr_core::{AgentId, AlertKind, SinkId};
pub use tokenmtr_export::SinkState;
use tokenmtr_export::{HEALTH_RUNS, HealthInput, sink_status, total_pending};
use tokenmtr_store::{Dataset, IssueKind, SinkRun, StoreRead, StoreReader};

use crate::context::QueryContext;
use crate::engine::{closed, to_count};
use crate::error::QueryError;
use crate::range::DateRange;

fn micros_ts(us: i64) -> Result<Timestamp, QueryError> {
    Timestamp::from_microsecond(us).map_err(|_| QueryError::decode("timestamp", us))
}

fn opt_ts(us: Option<i64>) -> Result<Option<Timestamp>, QueryError> {
    us.map(micros_ts).transpose()
}

/// A configured sink, as far as health is concerned (the caller maps its config to this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkSpec {
    /// Sink id.
    pub id: SinkId,
    /// How often it exports.
    pub interval: SignedDuration,
    /// Datasets it exports.
    pub datasets: Vec<Dataset>,
    /// Whether it is switched on.
    pub enabled: bool,
}

/// One export batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SinkRunView {
    /// Dataset exported.
    pub dataset: Dataset,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Timestamp,
    /// Rows exported.
    pub rows: u64,
    /// Error, absent on success.
    pub error: Option<String>,
}

impl From<&SinkRun> for SinkRunView {
    fn from(run: &SinkRun) -> Self {
        Self {
            dataset: run.dataset,
            started_at: run.started,
            finished_at: run.finished,
            rows: run.rows,
            error: run.error.clone(),
        }
    }
}

/// Health of one sink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SinkHealth {
    /// Sink id.
    pub sink: SinkId,
    /// State, computed by [`tokenmtr_export::sink_status`] like the daemon does.
    #[serde(flatten)]
    pub state: SinkState,
    /// Rows not yet exported, over all its datasets.
    pub lag_rows: u64,
    /// Age in seconds of the oldest unexported row.
    pub lag_secs: Option<i64>,
    /// End of the latest successful run.
    pub last_success_at: Option<Timestamp>,
    /// The latest runs, newest first (at most [`RECENT_RUNS`]).
    pub recent_runs: Vec<SinkRunView>,
}

/// Runs listed in [`SinkHealth::recent_runs`].
pub const RECENT_RUNS: usize = 10;

/// Health of every spec.
///
/// # Errors
///
/// Store failures.
pub fn sink_health(
    reader: &StoreReader,
    ctx: &QueryContext,
    specs: &[SinkSpec],
) -> Result<Vec<SinkHealth>, QueryError> {
    specs
        .iter()
        .map(|spec| one_sink(reader, ctx, spec))
        .collect()
}

fn one_sink(
    reader: &StoreReader,
    ctx: &QueryContext,
    spec: &SinkSpec,
) -> Result<SinkHealth, QueryError> {
    let runs = reader.sink_runs(&spec.id, HEALTH_RUNS)?;
    let pending = spec
        .datasets
        .iter()
        .map(|dataset| reader.pending(*dataset, reader.sink_cursor(&spec.id, *dataset)?))
        .collect::<Result<Vec<_>, _>>()?;
    let status = sink_status(&HealthInput {
        enabled: spec.enabled,
        interval: spec.interval,
        now: ctx.now(),
        runs: &runs,
        pending: total_pending(pending),
    });
    Ok(SinkHealth {
        sink: spec.id.clone(),
        state: status.state,
        lag_rows: status.lag.rows,
        lag_secs: status.lag.age.map(|age| age.as_secs()),
        last_success_at: runs.iter().find(|r| r.error.is_none()).map(|r| r.finished),
        recent_runs: runs
            .iter()
            .take(RECENT_RUNS)
            .map(SinkRunView::from)
            .collect(),
    })
}

/// Whether the daemon is alive, from its heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    /// A recent heartbeat.
    Running,
    /// The heartbeat is older than the staleness limit.
    Stale,
    /// No heartbeat was ever written.
    NeverStarted,
}

/// Daemon liveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DaemonHealth {
    /// State.
    pub state: DaemonState,
    /// Last heartbeat.
    pub last_heartbeat: Option<Timestamp>,
    /// Seconds since the last heartbeat.
    pub age_secs: Option<i64>,
}

/// Daemon liveness against `stale_after`.
///
/// # Errors
///
/// Store failures.
pub fn daemon_health(
    reader: &StoreReader,
    ctx: &QueryContext,
    stale_after: SignedDuration,
) -> Result<DaemonHealth, QueryError> {
    let beat = reader.heartbeat()?;
    let age = beat.map(|b| (ctx.now().as_second() - b.as_second()).max(0));
    let state = match age {
        None => DaemonState::NeverStarted,
        Some(a) if a > stale_after.as_secs() => DaemonState::Stale,
        Some(_) => DaemonState::Running,
    };
    Ok(DaemonHealth {
        state,
        last_heartbeat: beat,
        age_secs: age,
    })
}

/// The latest ingest run of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IngestStatus {
    /// Agent.
    pub agent: AgentId,
    /// End of the latest run.
    pub last_run_at: Timestamp,
    /// Files parsed in it.
    pub files_parsed: u64,
    /// Lines read.
    pub lines_read: u64,
    /// Events found.
    pub events_found: u64,
    /// Events that changed the store.
    pub events_changed: u64,
    /// Unresolved parse issues of the agent.
    pub open_issues: u64,
}

/// The latest ingest run per agent, with open-issue counts.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn ingest_status(reader: &StoreReader) -> Result<Vec<IngestStatus>, QueryError> {
    let mut statement = reader.connection().prepare(
        "SELECT r.agent, r.finished_at_us, r.files_parsed, r.lines_read, r.events_found, \
         r.events_changed, (SELECT COUNT(*) FROM parse_issue p WHERE p.agent = r.agent \
         AND p.resolved_at_us IS NULL) FROM ingest_run r WHERE r.id = \
         (SELECT id FROM ingest_run WHERE agent = r.agent ORDER BY started_at_us DESC, id DESC LIMIT 1) \
         ORDER BY r.agent",
    )?;
    let mut rows = statement.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let agent: String = row.get(0)?;
        let [files, lines, found, changed, open]: [i64; 5] = [
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
        ];
        out.push(IngestStatus {
            agent: agent
                .parse()
                .map_err(|_| QueryError::decode("agent", &agent))?,
            last_run_at: micros_ts(row.get(1)?)?,
            files_parsed: to_count(files)?,
            lines_read: to_count(lines)?,
            events_found: to_count(found)?,
            events_changed: to_count(changed)?,
            open_issues: to_count(open)?,
        });
    }
    Ok(out)
}

/// A parse issue (format drift, unknown version, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParseIssueView {
    /// Agent.
    pub agent: AgentId,
    /// What went wrong.
    pub kind: IssueKind,
    /// What it is about (file, version, …).
    pub subject: String,
    /// How often it was seen.
    pub occurrences: u64,
    /// A redacted sample.
    pub sample: Option<String>,
    /// First seen.
    pub first_seen: Timestamp,
    /// Last seen.
    pub last_seen: Timestamp,
    /// When it was resolved.
    pub resolved_at: Option<Timestamp>,
}

/// Parse issues, most recent first (at most `limit`); resolved ones only on request.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn parse_issues(
    reader: &StoreReader,
    include_resolved: bool,
    limit: u32,
) -> Result<Vec<ParseIssueView>, QueryError> {
    let mut statement = reader.connection().prepare(
        "SELECT agent, kind, subject, occurrences, sample, first_seen_us, last_seen_us, \
         resolved_at_us FROM parse_issue WHERE (? OR resolved_at_us IS NULL) \
         ORDER BY last_seen_us DESC, id DESC LIMIT ?",
    )?;
    let mut rows = statement.query([i64::from(include_resolved), i64::from(limit)])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let occurrences: i64 = row.get(3)?;
        let agent: String = row.get(0)?;
        out.push(ParseIssueView {
            agent: agent
                .parse()
                .map_err(|_| QueryError::decode("agent", &agent))?,
            kind: closed("issue kind", &row.get::<_, String>(1)?)?,
            subject: row.get(2)?,
            occurrences: to_count(occurrences)?,
            sample: row.get(4)?,
            first_seen: micros_ts(row.get(5)?)?,
            last_seen: micros_ts(row.get(6)?)?,
            resolved_at: opt_ts(row.get(7)?)?,
        });
    }
    Ok(out)
}

/// A fired alert (D36).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlertView {
    /// When it fired.
    pub fired_at: Timestamp,
    /// Its dedup key.
    pub dedup_key: String,
    /// What it is about.
    #[serde(flatten)]
    pub alert: AlertKind,
}

/// Alerts fired at or after `since`, newest first (at most `limit`).
///
/// # Errors
///
/// Store, SQL or decode failures (a payload that is not an [`AlertKind`]).
pub fn alerts(
    reader: &StoreReader,
    since: Timestamp,
    limit: u32,
) -> Result<Vec<AlertView>, QueryError> {
    let mut statement = reader.connection().prepare(
        "SELECT fired_at_us, dedup_key, payload FROM alert WHERE fired_at_us >= ? \
         ORDER BY fired_at_us DESC, id DESC LIMIT ?",
    )?;
    let mut rows = statement.query(params_from_iter([
        Value::Integer(since.as_microsecond()),
        Value::Integer(i64::from(limit)),
    ]))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let payload: String = row.get(2)?;
        out.push(AlertView {
            fired_at: micros_ts(row.get(0)?)?,
            dedup_key: row.get(1)?,
            alert: serde_json::from_str(&payload)
                .map_err(|_| QueryError::decode("alert payload", payload))?,
        });
    }
    Ok(out)
}

/// One backup attempt (D37).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupView {
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Timestamp,
    /// Destination file.
    pub path: Option<String>,
    /// Size of the backup.
    pub size_bytes: Option<u64>,
    /// Error, absent on success.
    pub error: Option<String>,
}

/// The latest backup attempts, newest first.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn backup_runs(reader: &StoreReader, limit: u32) -> Result<Vec<BackupView>, QueryError> {
    let mut statement = reader.connection().prepare(
        "SELECT started_at_us, finished_at_us, path, size_bytes, error FROM backup_run \
         ORDER BY started_at_us DESC, id DESC LIMIT ?",
    )?;
    let mut rows = statement.query([i64::from(limit)])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let size: Option<i64> = row.get(3)?;
        out.push(BackupView {
            started_at: micros_ts(row.get(0)?)?,
            finished_at: micros_ts(row.get(1)?)?,
            path: row.get(2)?,
            size_bytes: size.map(to_count).transpose()?,
            error: row.get(4)?,
        });
    }
    Ok(out)
}

/// One stored ECB rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FxRateView {
    /// Publication date (UTC).
    pub date: Date,
    /// EUR per 1 USD × 10⁹.
    pub eur_per_usd_scaled: i64,
    /// Where it came from.
    pub source: String,
}

/// Stored FX rates dated inside `range`, oldest first.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn fx_rates(reader: &StoreReader, range: DateRange) -> Result<Vec<FxRateView>, QueryError> {
    let mut statement = reader.connection().prepare(
        "SELECT date, eur_per_usd_scaled, source FROM fx_rate WHERE date >= ? AND date < ? \
         ORDER BY date",
    )?;
    let mut rows = statement.query([range.start().to_string(), range.end().to_string()])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let date: String = row.get(0)?;
        out.push(FxRateView {
            date: date
                .parse()
                .map_err(|_| QueryError::decode("fx date", &date))?,
            eur_per_usd_scaled: row.get(1)?,
            source: row.get(2)?,
        });
    }
    Ok(out)
}
