//! Opening the database and the public API of the store.
//!
//! [`Store`] is the single writer: the daemon holds one and funnels every write through it.
//! [`StoreReader`] is a read-only connection for the query layer and the CLI. Both implement
//! [`StoreRead`], which holds every read the writers and exporters need.

use std::collections::HashSet;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use tokenmtr_core::{
    AgentId, Alert, DedupKey, EventId, FxRate, MachineId, PricingVersion, ProfileId, SinkId,
    SourceFileId, Subscription, UsageEvent,
};

use crate::account::{self, AccountRecord};
use crate::alert;
use crate::backup::{self, BackupInfo};
use crate::checkpoint::{self, Checkpoint, StoredCheckpoint};
use crate::error::StoreError;
use crate::events;
use crate::fx::{self, FxRateRow, FxSource};
use crate::ingest::{self, IngestWrite, IngestWriteReport};
use crate::issue::{self, IssueKind, OpenParseIssue, ParseIssue};
use crate::meta::{self, Identity};
use crate::migrations::{SCHEMA_VERSION, migrations};
use crate::pricing::{self, StoredPricingSnapshot};
use crate::project::{self, ProjectResolution};
use crate::quota::{self, QuotaPoll, QuotaSample};
use crate::runs::{self, BackupRun, IngestRun};
use crate::signal::{self, SessionSignal};
use crate::sink::{self, SinkRun};
use crate::subscriptions;
use crate::tool_calls::{self, ProfiledToolCall, ResultOutcome};
use crate::types::{
    Dataset, Page, PageCursor, Pending, RedactionKey, Revision, StoredEvent, StoredSubscription,
    UpsertStats, Versioned,
};
use crate::versioned;

mod pricing_api;

/// How long a statement waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Memory-mapped I/O window of every connection.
const MMAP_SIZE: i64 = 268_435_456;

/// WAL pages before an automatic checkpoint.
const WAL_AUTOCHECKPOINT: i64 = 4000;

fn user_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Refuses a database written by a newer build.
pub(crate) fn reject_newer_schema(conn: &Connection) -> Result<u32, StoreError> {
    let found = user_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::NewerSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(found)
}

/// Takes an exclusive advisory lock on `<db>-lock`; it is held while the returned file lives,
/// which a connection setup keeps for its duration.
///
/// The app, the CLI and the daemon may all open a fresh database at once, and `SQLite`'s own locks
/// do not serialise that: `to_latest` reads `user_version` before its transaction (`table meta
/// already exists`), and switching to WAL can report `database is locked`. The lock goes when the
/// file closes, also when the process dies.
fn lock_setup(db: &Utf8Path) -> Result<std::fs::File, StoreError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(format!("{db}-lock"))?;
    file.lock()?;
    Ok(file)
}

/// Runs `PRAGMA integrity_check`; anything but a single `ok` is an error.
pub(crate) fn check_integrity(conn: &Connection) -> Result<(), StoreError> {
    let mut statement = conn.prepare("PRAGMA integrity_check")?;
    let lines = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if lines == ["ok"] {
        Ok(())
    } else {
        Err(StoreError::IntegrityCheck(lines.join("; ")))
    }
}

mod sealed {
    use rusqlite::Connection;

    use crate::meta::Identity;

    /// Gives [`StoreRead`](super::StoreRead) its connection without exposing it.
    pub trait Sealed {
        fn connection(&self) -> &Connection;
        fn identity(&self) -> &Identity;
    }
}

/// The reads shared by [`Store`] and [`StoreReader`].
///
/// Sealed: only this crate implements it.
pub trait StoreRead: sealed::Sealed {
    /// The id of this machine, generated once when the database was created.
    fn machine_id(&self) -> MachineId {
        self.identity().machine_id
    }

    /// The key redaction hashes with.
    fn redaction_key(&self) -> &RedactionKey {
        &self.identity().redaction_key
    }

    /// `PRAGMA user_version`.
    ///
    /// # Errors
    /// `SQLite` failures.
    fn schema_version(&self) -> Result<u32, StoreError> {
        user_version(self.connection())
    }

    /// When the daemon last called [`Store::set_heartbeat`].
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt value.
    fn heartbeat(&self) -> Result<Option<Timestamp>, StoreError> {
        meta::heartbeat(self.connection())
    }

    /// The newest revision of a dataset; [`Revision::ZERO`] when it is empty.
    ///
    /// # Errors
    /// `SQLite` failures.
    fn head_revision(&self, dataset: Dataset) -> Result<Revision, StoreError> {
        versioned::head(self.connection(), dataset)
    }

    /// What a cursor at `after` has yet to export: row count and age of the oldest row.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt value.
    fn pending(&self, dataset: Dataset, after: Revision) -> Result<Pending, StoreError> {
        versioned::pending(self.connection(), dataset, after)
    }

    /// The last revision `sink` acknowledged for `dataset`; [`Revision::ZERO`] before the first.
    ///
    /// # Errors
    /// `SQLite` failures.
    fn sink_cursor(&self, sink: &SinkId, dataset: Dataset) -> Result<Revision, StoreError> {
        sink::cursor(self.connection(), sink, dataset)
    }

    /// Up to `limit` events with a revision above `after`, in revision order, superseded ones
    /// included (see [`StoreRead::stored_events_after`] for the flag).
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn usage_events_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<UsageEvent>>, StoreError> {
        Ok(self
            .stored_events_after(after, limit)?
            .into_iter()
            .map(|v| Versioned {
                revision: v.revision,
                row: v.row.event,
            })
            .collect())
    }

    /// [`StoreRead::usage_events_after`] with the store-side state of each row: what sinks
    /// export.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn stored_events_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<StoredEvent>>, StoreError> {
        events::after(self.connection(), after, limit)
    }

    /// Up to `limit` tool calls with a revision above `after`, in revision order.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn tool_calls_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<ProfiledToolCall>>, StoreError> {
        tool_calls::after(self.connection(), after, limit)
    }

    /// Up to `limit` subscriptions with a revision above `after`, in revision order.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn subscriptions_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<StoredSubscription>>, StoreError> {
        subscriptions::after(self.connection(), after, limit)
    }

    /// Every subscription still in the config (not removed), by id: what billing uses.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn active_subscriptions(&self) -> Result<Vec<Subscription>, StoreError> {
        subscriptions::active(self.connection())
    }

    /// Up to `limit` quota rows with a revision above `after`, in revision order.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn quota_snapshots_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<QuotaSample>>, StoreError> {
        quota::after(self.connection(), after, limit)
    }

    /// Up to `limit` FX rates with a revision above `after`, in revision order.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn fx_rates_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<FxRateRow>>, StoreError> {
        fx::after(self.connection(), after, limit)
    }

    /// The event stored under `(agent, dedup_key)`.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn usage_event_by_key(
        &self,
        agent: AgentId,
        dedup_key: &DedupKey,
    ) -> Result<Option<Versioned<UsageEvent>>, StoreError> {
        events::by_key(self.connection(), agent, dedup_key)
    }

    /// The event with this id.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn usage_event(&self, id: &EventId) -> Result<Option<Versioned<UsageEvent>>, StoreError> {
        events::by_id(self.connection(), id)
    }

    /// Up to `limit` events of one working directory in insertion order, resuming at `after`.
    /// Revisions may change between pages (see [`Store::reassign_projects`]); the order does not.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn usage_events_by_cwd(
        &self,
        cwd: &Utf8Path,
        after: Option<PageCursor>,
        limit: usize,
    ) -> Result<Page<Versioned<UsageEvent>>, StoreError> {
        let rows = events::by_cwd(self.connection(), cwd, after.map_or(0, |c| c.0), limit)?;
        let next = if rows.len() == limit {
            rows.last().map(|(id, _)| PageCursor(*id))
        } else {
            None
        };
        Ok(Page {
            items: rows.into_iter().map(|(_, event)| event).collect(),
            next,
        })
    }

    /// The tool call stored under `(agent, tool_use_id)`.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn tool_call(
        &self,
        agent: AgentId,
        tool_use_id: &str,
    ) -> Result<Option<Versioned<ProfiledToolCall>>, StoreError> {
        tool_calls::by_key(self.connection(), agent, tool_use_id)
    }

    /// When the first event of `(agent, profile)` that is not a stats-cache aggregate happened
    ///; `None` when there is none.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt value.
    fn earliest_transcript_event(
        &self,
        agent: AgentId,
        profile: &ProfileId,
    ) -> Result<Option<Timestamp>, StoreError> {
        events::earliest_transcript(self.connection(), agent, profile)
    }

    /// Every unresolved parse issue, oldest first.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn open_parse_issues(&self) -> Result<Vec<OpenParseIssue>, StoreError> {
        issue::open(self.connection())
    }

    /// The newest ingest passes, newest first; of one agent when `agent` is given.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn ingest_runs(
        &self,
        agent: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<IngestRun>, StoreError> {
        runs::ingest_runs(self.connection(), agent, limit)
    }

    /// The newest export attempts of `sink`, newest first.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn sink_runs(&self, sink: &SinkId, limit: usize) -> Result<Vec<SinkRun>, StoreError> {
        sink::runs(self.connection(), sink, limit)
    }

    /// The working directories of stored events that have no resolution yet, ordered.
    ///
    /// # Errors
    /// `SQLite` failures.
    fn unresolved_cwds(&self) -> Result<Vec<Utf8PathBuf>, StoreError> {
        project::unresolved(self.connection())
    }

    /// Every checkpoint of one agent, ordered by path.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn load_checkpoints(&self, agent: AgentId) -> Result<Vec<StoredCheckpoint>, StoreError> {
        checkpoint::load(self.connection(), agent)
    }

    /// The resolution of one working directory.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn project(&self, cwd: &Utf8Path) -> Result<Option<ProjectResolution>, StoreError> {
        project::get(self.connection(), cwd)
    }

    /// Every working directory resolution, ordered by directory.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn projects(&self) -> Result<Vec<ProjectResolution>, StoreError> {
        project::all(self.connection())
    }

    /// Every known account, ordered by agent and profile.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        account::all(self.connection())
    }

    /// The pricing snapshot in use, once one has been stored.
    ///
    /// # Errors
    /// `SQLite` failures, a corrupt body or a body that does not match its recorded length.
    fn active_pricing_snapshot(&self) -> Result<Option<StoredPricingSnapshot>, StoreError> {
        pricing::active(self.connection())
    }

    /// The version of the pricing snapshot in use, without reading its body: a cheap check of
    /// whether a parsed catalog is still current.
    ///
    /// # Errors
    /// `SQLite` failures or a stored version that is not 12 hex characters.
    fn active_pricing_version(&self) -> Result<Option<PricingVersion>, StoreError> {
        pricing::active_version(self.connection())
    }

    /// The dedup keys of every alert fired so far.
    ///
    /// # Errors
    /// `SQLite` failures.
    fn fired_alert_keys(&self) -> Result<HashSet<String>, StoreError> {
        alert::fired_keys(self.connection())
    }

    /// Writes a zstd-compressed, integrity-checked snapshot to `dest`, atomically.
    ///
    /// Safe while the writer is busy: the snapshot holds the state of the last commit.
    ///
    /// # Errors
    /// `SQLite` or file failures, or a snapshot that fails `PRAGMA integrity_check`.
    fn backup(&self, dest: &Utf8Path) -> Result<BackupInfo, StoreError> {
        backup::backup(self.connection(), dest)
    }
}

/// The writer connection. At most one process may hold it.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    identity: Identity,
    clock: fn() -> Timestamp,
}

impl Store {
    /// Opens the database at `path`, creating and migrating it as needed.
    ///
    /// # Errors
    /// [`StoreError::NewerSchema`] when a newer build wrote the file (it is left untouched);
    /// [`StoreError::WalUnavailable`] when the file system refuses write-ahead logging; `SQLite`,
    /// migration and file failures.
    pub fn open(path: &Utf8Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let _setup_lock = lock_setup(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        reject_newer_schema(&conn)?;
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::WalUnavailable(mode));
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "mmap_size", MMAP_SIZE)?;
        conn.pragma_update(None, "wal_autocheckpoint", WAL_AUTOCHECKPOINT)?;
        // macOS ignores plain fsync; make commits and checkpoints durable.
        conn.pragma_update(None, "fullfsync", true)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true)?;
        migrations().to_latest(&mut conn)?;
        let identity = meta::load_identity(&conn)?;
        Ok(Self {
            conn,
            identity,
            clock: Timestamp::now,
        })
    }

    /// Replaces the clock that stamps `updated_at` columns, checkpoints and heartbeats.
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> Timestamp) -> Self {
        self.clock = clock;
        self
    }

    /// Inserts or updates events by `(agent, dedup_key)`, last write wins.
    ///
    /// All rows go in one transaction. A row gets a new revision only when one of its values
    /// changes; identical rows keep theirs. When a batch repeats a key, its last row counts.
    /// `source_file` ids must come from [`Store::save_checkpoint`].
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back) or values outside `SQLite`'s integers.
    pub fn upsert_events(&mut self, events: &[UsageEvent]) -> Result<UpsertStats, StoreError> {
        self.in_transaction(|conn, now| events::upsert(conn, now, events))
    }

    /// Inserts or updates tool calls by `(agent, tool_use_id)`. The event link and the error flag
    /// are never erased by a later row that lacks them.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_tool_calls(
        &mut self,
        calls: &[ProfiledToolCall],
    ) -> Result<UpsertStats, StoreError> {
        self.in_transaction(|conn, now| tool_calls::upsert(conn, now, calls))
    }

    /// Mirrors the configured subscriptions by id: `subscriptions` is the whole list.
    /// Rows missing from it are kept with `removed_at` set (counted as updated); a removed id
    /// that comes back is live again.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn mirror_subscriptions(
        &mut self,
        subscriptions: &[Subscription],
    ) -> Result<UpsertStats, StoreError> {
        self.in_transaction(|conn, now| subscriptions::mirror(conn, now, subscriptions))
    }

    /// Stores quota polls, one row per reported window.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_quota_snapshots(
        &mut self,
        polls: &[QuotaPoll],
    ) -> Result<UpsertStats, StoreError> {
        self.in_transaction(|conn, now| quota::upsert(conn, now, polls))
    }

    /// Stores FX rates by date.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_fx_rates(
        &mut self,
        source: FxSource,
        rates: &[FxRate],
    ) -> Result<UpsertStats, StoreError> {
        self.in_transaction(|conn, now| fx::upsert(conn, now, source, rates))
    }

    /// Stores session signals not seen before; returns how many were new.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn record_signals(&mut self, signals: &[SessionSignal]) -> Result<u64, StoreError> {
        self.in_transaction(|conn, _| signal::record(conn, signals))
    }

    /// Sets the error flag of a stored tool call from its tool result. The call gets a new
    /// revision only when the flag changes. Returns whether it did; `false` also when no such
    /// call is stored.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn apply_tool_result(
        &mut self,
        agent: AgentId,
        tool_use_id: &str,
        is_error: bool,
    ) -> Result<bool, StoreError> {
        let outcome = self.in_transaction(|conn, now| {
            tool_calls::apply_result(conn, now, agent, tool_use_id, is_error)
        })?;
        Ok(outcome == ResultOutcome::Applied)
    }

    /// Writes one ingest flush in a single transaction: events, tool calls, signals, tool-result
    /// flags, parse-issue changes and file checkpoints. Either all of it is stored or,
    /// after an error or a crash, none of it, so a checkpoint never runs ahead of its rows.
    ///
    /// # Errors
    /// `SQLite` failures or values outside `SQLite`'s integers (the transaction is rolled back).
    pub fn commit_ingest(
        &mut self,
        write: &IngestWrite<'_>,
    ) -> Result<IngestWriteReport, StoreError> {
        self.in_transaction(|conn, now| ingest::apply(conn, now, write))
    }

    /// Saves `resolutions` and points the events of each working directory at its project
    /// and client, in one transaction. Only events whose project or client differ are rewritten;
    /// each gets a new revision, so sinks learn of exactly those. Returns how many changed.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn reassign_projects(
        &mut self,
        resolutions: &[ProjectResolution],
    ) -> Result<u64, StoreError> {
        self.in_transaction(|conn, now| project::reassign(conn, now, resolutions))
    }

    /// Flags as superseded the stats-cache daily aggregates of `(agent, profile)` that
    /// transcript events now cover: those on or after the local day (in `zone`) of the
    /// profile's earliest transcript event. Rows are kept; each newly flagged row gets a new
    /// revision, so sinks re-export it with `superseded = true`, and every query skips it.
    /// Returns how many rows were newly flagged; none when the profile has no transcript events.
    ///
    /// # Errors
    /// `SQLite` failures or a day outside the supported range (the transaction is rolled back).
    pub fn supersede_daily_aggregates_covered_by_transcripts(
        &mut self,
        agent: AgentId,
        profile: &ProfileId,
        zone: &jiff::tz::TimeZone,
    ) -> Result<u64, StoreError> {
        self.in_transaction(|conn, now| {
            let Some(first) = events::earliest_transcript(conn, agent, profile)? else {
                return Ok(0);
            };
            let from = first.to_zoned(zone.clone()).start_of_day()?.timestamp();
            events::supersede_aggregates_from(conn, now, agent, profile, from)
        })
    }

    /// Stores a fired alert; `false` when its dedup key was already stored.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn save_alert(&mut self, alert: &Alert) -> Result<bool, StoreError> {
        alert::save(&self.conn, alert)
    }

    /// Appends one export attempt to the sink history. The history keeps the newest 1000
    /// runs per sink. Cursors and runs of a sink removed from the config stay, so a sink re-added
    /// under the same id resumes where it stopped.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn record_sink_run(&mut self, run: &SinkRun) -> Result<(), StoreError> {
        self.in_transaction(|conn, _| sink::record_run(conn, run, sink::RUN_HISTORY))
    }

    /// Appends one backup attempt.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn record_backup_run(&mut self, run: &BackupRun) -> Result<(), StoreError> {
        runs::record_backup(&self.conn, run)
    }

    /// Appends one ingest pass.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn record_ingest_run(&mut self, run: &IngestRun) -> Result<(), StoreError> {
        runs::record_ingest(&self.conn, run)
    }

    /// Counts an occurrence of a parse issue.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn record_parse_issue(&mut self, issue: &ParseIssue) -> Result<(), StoreError> {
        issue::record(&self.conn, issue)
    }

    /// Resolves an open parse issue; `false` when there was none.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn close_parse_issue(
        &mut self,
        agent: AgentId,
        kind: IssueKind,
        subject: &str,
        at: Timestamp,
    ) -> Result<bool, StoreError> {
        issue::close(&self.conn, agent, kind, subject, at)
    }

    /// Stores a pricing body, compressed, and makes it the active snapshot. Storing a body
    /// that is already stored only re-activates it.
    ///
    /// # Errors
    /// `SQLite` or compression failures.
    pub fn put_pricing_snapshot(
        &mut self,
        body: &[u8],
        fetched_at: Timestamp,
        source: Option<&str>,
    ) -> Result<PricingVersion, StoreError> {
        pricing::put(&mut self.conn, body, fetched_at, source)
    }

    /// Inserts or replaces the checkpoint of its path and returns the id events reference it by
    /// (`UsageEvent::source_file`). Call it with offset 0 to obtain the id before parsing.
    ///
    /// # Errors
    /// `SQLite` failures or values outside `SQLite`'s integers.
    pub fn save_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<SourceFileId, StoreError> {
        checkpoint::save(&self.conn, (self.clock)(), checkpoint)
    }

    /// Sets the revision a sink has acknowledged. Moving it backwards re-delivers rows, but
    /// sinks drop rows they already hold (an append file skips revisions at or below its highest,
    /// `SQLite` keeps the higher revision): to rebuild a sink, also delete its output.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn set_sink_cursor(
        &mut self,
        sink: &SinkId,
        dataset: Dataset,
        revision: Revision,
    ) -> Result<(), StoreError> {
        sink::set_cursor(&self.conn, sink, dataset, revision)
    }

    /// Records that the daemon is alive now.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn set_heartbeat(&mut self) -> Result<(), StoreError> {
        meta::set_heartbeat(&self.conn, (self.clock)())
    }

    /// Inserts or replaces the resolution of its working directory.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn save_project(&mut self, resolution: &ProjectResolution) -> Result<(), StoreError> {
        project::save(&self.conn, resolution)
    }

    /// Inserts or replaces the account of an agent profile.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn save_account(&mut self, account: &AccountRecord) -> Result<(), StoreError> {
        account::save(&self.conn, account)
    }

    /// Runs `body` in one immediate write transaction with the clock read once, and commits when
    /// it returns `Ok`.
    fn in_transaction<T>(
        &mut self,
        body: impl FnOnce(&Connection, Timestamp) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let now = (self.clock)();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = body(&tx, now)?;
        tx.commit()?;
        Ok(value)
    }

    /// The connection, for tests that need raw SQL.
    #[cfg(test)]
    pub(crate) fn raw(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl sealed::Sealed for Store {
    fn connection(&self) -> &Connection {
        &self.conn
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }
}

impl StoreRead for Store {}

/// A read-only connection to a database a [`Store`] has created and migrated.
///
/// It never blocks the writer and sees the state of the last commit at the time of each query.
#[derive(Debug)]
pub struct StoreReader {
    conn: Connection,
    identity: Identity,
}

impl StoreReader {
    /// Opens the database at `path` read-only.
    ///
    /// # Errors
    /// [`StoreError::NewerSchema`] / [`StoreError::OlderSchema`] when the schema is not exactly
    /// this build's, [`StoreError::MissingMeta`] when the file is not a store, `SQLite` failures
    /// (including a missing file).
    pub fn open(path: &Utf8Path) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let found = reject_newer_schema(&conn)?;
        if found < SCHEMA_VERSION {
            return Err(StoreError::OlderSchema {
                found,
                expected: SCHEMA_VERSION,
            });
        }
        conn.pragma_update(None, "query_only", true)?;
        conn.pragma_update(None, "mmap_size", MMAP_SIZE)?;
        let identity = meta::load_identity(&conn)?;
        Ok(Self { conn, identity })
    }

    /// The connection, for the query layer's own SQL. It is read-only.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.conn
    }
}

impl sealed::Sealed for StoreReader {
    fn connection(&self) -> &Connection {
        &self.conn
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }
}

impl StoreRead for StoreReader {}
