//! Opening the database and the public API of the store.
//!
//! [`Store`] is the single writer: the daemon holds one and funnels every write through it.
//! [`StoreReader`] is a read-only connection for the query layer and the CLI. Both implement
//! [`StoreRead`], which holds every read the writers and exporters need.

use std::collections::HashSet;
use std::time::Duration;

use camino::Utf8Path;
use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags};
use tokenmtr_core::{
    AgentId, Alert, FxRate, MachineId, PricingVersion, SinkId, SourceFileId, Subscription,
    UsageEvent,
};

use crate::account::{self, AccountRecord};
use crate::alert;
use crate::backup::{self, BackupInfo};
use crate::checkpoint::{self, Checkpoint, StoredCheckpoint};
use crate::error::StoreError;
use crate::events;
use crate::fx::{self, FxRateRow, FxSource};
use crate::issue::{self, IssueKind, ParseIssue};
use crate::meta::{self, Identity};
use crate::migrations::{SCHEMA_VERSION, migrations};
use crate::pricing::{self, StoredPricingSnapshot};
use crate::project::{self, ProjectResolution};
use crate::quota::{self, QuotaPoll, QuotaSample};
use crate::runs::{self, BackupRun, IngestRun};
use crate::signal::{self, SessionSignal};
use crate::sink::{self, SinkRun};
use crate::subscriptions;
use crate::tool_calls::{self, ProfiledToolCall};
use crate::types::{Dataset, Pending, RedactionKey, Revision, UpsertStats, Versioned};
use crate::versioned;

/// How long a statement waits for another connection's lock (D6).
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Memory-mapped I/O window of every connection (D6).
const MMAP_SIZE: i64 = 268_435_456;

/// WAL pages before an automatic checkpoint (D6).
const WAL_AUTOCHECKPOINT: i64 = 4000;

fn user_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Refuses a database written by a newer build (D52).
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

/// Runs `PRAGMA integrity_check`; anything but a single `ok` is an error (D37).
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
    /// The id of this machine, generated once when the database was created (D30).
    fn machine_id(&self) -> MachineId {
        self.identity().machine_id
    }

    /// The key redaction hashes with (D33).
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

    /// What a cursor at `after` has yet to export: row count and age of the oldest row (D41).
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

    /// Up to `limit` events with a revision above `after`, in revision order.
    ///
    /// # Errors
    /// `SQLite` failures or a corrupt row.
    fn usage_events_after(
        &self,
        after: Revision,
        limit: usize,
    ) -> Result<Vec<Versioned<UsageEvent>>, StoreError> {
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
    ) -> Result<Vec<Versioned<Subscription>>, StoreError> {
        subscriptions::after(self.connection(), after, limit)
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

    /// The dedup keys of every alert fired so far (D36).
    ///
    /// # Errors
    /// `SQLite` failures.
    fn fired_alert_keys(&self) -> Result<HashSet<String>, StoreError> {
        alert::fired_keys(self.connection())
    }

    /// Writes a zstd-compressed, integrity-checked snapshot to `dest`, atomically (D37).
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
        // D49: macOS ignores plain fsync; make commits and checkpoints durable.
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

    /// Inserts or updates events by `(agent, dedup_key)`, last write wins (D12).
    ///
    /// All rows go in one transaction. A row gets a new revision only when one of its values
    /// changes; identical rows keep theirs. When a batch repeats a key, its last row counts.
    /// `source_file` ids must come from [`Store::save_checkpoint`].
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back) or values outside `SQLite`'s integers.
    pub fn upsert_events(&mut self, events: &[UsageEvent]) -> Result<UpsertStats, StoreError> {
        events::upsert(&mut self.conn, (self.clock)(), events)
    }

    /// Inserts or updates tool calls by `(agent, tool_use_id)`. The event link and the error flag
    /// are never erased by a later row that lacks them (D32).
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_tool_calls(
        &mut self,
        calls: &[ProfiledToolCall],
    ) -> Result<UpsertStats, StoreError> {
        tool_calls::upsert(&mut self.conn, (self.clock)(), calls)
    }

    /// Mirrors subscriptions by id (D28). Subscriptions missing from `subscriptions` are kept.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_subscriptions(
        &mut self,
        subscriptions: &[Subscription],
    ) -> Result<UpsertStats, StoreError> {
        subscriptions::upsert(&mut self.conn, (self.clock)(), subscriptions)
    }

    /// Stores quota polls, one row per reported window (D26).
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_quota_snapshots(
        &mut self,
        polls: &[QuotaPoll],
    ) -> Result<UpsertStats, StoreError> {
        quota::upsert(&mut self.conn, (self.clock)(), polls)
    }

    /// Stores FX rates by date (D27).
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn upsert_fx_rates(
        &mut self,
        source: FxSource,
        rates: &[FxRate],
    ) -> Result<UpsertStats, StoreError> {
        fx::upsert(&mut self.conn, (self.clock)(), source, rates)
    }

    /// Stores session signals not seen before; returns how many were new.
    ///
    /// # Errors
    /// `SQLite` failures (the transaction is rolled back).
    pub fn record_signals(&mut self, signals: &[SessionSignal]) -> Result<u64, StoreError> {
        signal::record(&mut self.conn, signals)
    }

    /// Stores a fired alert; `false` when its dedup key was already stored (D36).
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn save_alert(&mut self, alert: &Alert) -> Result<bool, StoreError> {
        alert::save(&self.conn, alert)
    }

    /// Appends one export attempt to the sink history (D41).
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn record_sink_run(&mut self, run: &SinkRun) -> Result<(), StoreError> {
        sink::record_run(&self.conn, run)
    }

    /// Appends one backup attempt (D37).
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

    /// Counts an occurrence of a parse issue (D42).
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

    /// Stores a pricing body, compressed, and makes it the active snapshot (D11). Storing a body
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

    /// Sets the revision a sink has acknowledged. Moving it backwards re-exports rows (D17).
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

    /// Inserts or replaces the resolution of its working directory (D31).
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn save_project(&mut self, resolution: &ProjectResolution) -> Result<(), StoreError> {
        project::save(&self.conn, resolution)
    }

    /// Inserts or replaces the account of an agent profile (D26).
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn save_account(&mut self, account: &AccountRecord) -> Result<(), StoreError> {
        account::save(&self.conn, account)
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
