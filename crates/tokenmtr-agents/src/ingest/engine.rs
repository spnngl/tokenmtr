//! The ingestor: discovery → checkpoint comparison → parse → price → resolve → store.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use jiff::{SignedDuration, Timestamp};
use rayon::prelude::*;
use tokenmtr_core::{
    AgentId, DedupKey, MachineId, ParsedEvent, SessionId, SourceFileId, UsageEvent,
};
use tokenmtr_platform::{ThreadPriority, set_thread_priority};
use tokenmtr_store::{
    Checkpoint, IngestRun, IngestWrite, IssueKind, IssueUpdate, ParseIssue as StoredIssue,
    ProfiledToolCall, SessionSignal, Store, StoreRead as _, StoreReader, StoredCheckpoint,
    ToolResultFlag,
};

use super::error::IngestError;
use super::fingerprint::{FileStat, ParseReason, Plan, decide, last_line_hash};
use super::pricing::Pricer;
use super::projects::{ProjectCache, Reresolve};
use super::report::{AgentScan, AggregateScan, FileScan, ScanReport};
use super::settings::IngestSettings;
use crate::batch::{BatchParts, ParseBatch};
use crate::project::ProjectResolver;
use crate::source::{ByteOffset, SourceFile};
use crate::{AgentKind, AgentSource, DriftReason, ParseIssue, ParseOutcome, discover};

/// Events per store transaction (D48).
const CHUNK: usize = 5000;

/// An unterminated last line older than this was left by a crashed writer: it is skipped as
/// malformed, so the next appended line is not glued to it.
const ABANDONED_TAIL: SignedDuration = SignedDuration::from_mins(10);

/// Subject of the per-scan drift issue (D42): the share of failed usage-bearing lines over every
/// file of one scan.
pub const SCAN_ISSUE_SUBJECT: &str = "scan";

/// Whether a scan trusts the stored checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Parse only what changed since the checkpoints.
    Incremental,
    /// Ignore the checkpoints: parse every file from zero and re-resolve every project. Stored
    /// rows that come out identical do not move (`scan --full`).
    Full,
}

/// How hard a scan may compete with the user's other work (D47).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPriority {
    /// A person waits for it (`scan`, first run, `doctor`): files parse in parallel at full
    /// priority.
    UserInitiated,
    /// The daemon's steady state: one file at a time, at background priority.
    Background,
}

/// What one scan does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequest {
    /// Agents to scan; `None` means every configured agent.
    pub agents: Option<Vec<AgentId>>,
    /// Checkpoint handling.
    pub mode: ScanMode,
    /// Scheduling class.
    pub priority: ScanPriority,
    /// Only these files (a watcher batch); `None` means discover everything. Files that no agent
    /// owns or that no longer exist are ignored. The stats-cache import is skipped.
    pub paths: Option<Vec<Utf8PathBuf>>,
}

impl ScanRequest {
    /// Every agent, only what changed.
    #[must_use]
    pub const fn incremental(priority: ScanPriority) -> Self {
        Self {
            agents: None,
            mode: ScanMode::Incremental,
            priority,
            paths: None,
        }
    }

    /// Every agent, every file from zero.
    #[must_use]
    pub const fn full(priority: ScanPriority) -> Self {
        Self {
            agents: None,
            mode: ScanMode::Full,
            priority,
            paths: None,
        }
    }

    /// Restricts the scan to `agent` (callable repeatedly to name several).
    #[must_use]
    pub fn for_agent(mut self, agent: AgentId) -> Self {
        self.agents.get_or_insert_with(Vec::new).push(agent);
        self
    }

    /// Restricts the scan to the given files.
    #[must_use]
    pub fn for_paths(mut self, paths: Vec<Utf8PathBuf>) -> Self {
        self.paths = Some(paths);
        self
    }
}

/// Reads agent files into the store.
///
/// The ingestor writes through the `&mut Store` of each [`scan`](Self::scan) and reads stored
/// facts (an event's estimate, a tool call) through its own read-only [`StoreReader`], which sees
/// every transaction the writer committed.
pub struct Ingestor {
    pub(super) settings: IngestSettings,
    pub(super) resolver: ProjectResolver,
    pricer: Pricer,
    pub(super) reader: StoreReader,
    clock: fn() -> Timestamp,
    /// The parse pool of user-initiated scans, built by the first one that needs it.
    pool: OnceLock<rayon::ThreadPool>,
    /// Whether this process already retried the cwds that fell back to themselves (D50): the
    /// first scan does, so a restart retries them; a full scan retries every cwd.
    fallbacks_retried: bool,
    #[cfg(test)]
    pub(crate) crash_before_commit: bool,
}

impl Ingestor {
    /// An ingestor for `settings` that reads stored facts through `reader`.
    #[must_use]
    pub fn new(settings: IngestSettings, reader: StoreReader) -> Self {
        Self {
            resolver: ProjectResolver::new(settings.rules.clone(), settings.protected.clone()),
            fallbacks_retried: false,
            pricer: Pricer::new(settings.overrides.clone()),
            settings,
            reader,
            clock: Timestamp::now,
            pool: OnceLock::new(),
            #[cfg(test)]
            crash_before_commit: false,
        }
    }

    /// Like [`new`](Self::new), opening the reader on the store file at `store_path`, which must
    /// exist (open the [`Store`] first).
    pub fn open(settings: IngestSettings, store_path: &Utf8Path) -> Result<Self, IngestError> {
        Ok(Self::new(settings, StoreReader::open(store_path)?))
    }

    /// Replaces the clock (tests).
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> Timestamp) -> Self {
        self.clock = clock;
        self
    }

    /// Every root of every profile: what the watcher watches.
    #[must_use]
    pub fn roots(&self) -> Vec<Utf8PathBuf> {
        let mut roots: Vec<Utf8PathBuf> = self
            .settings
            .sources
            .iter()
            .flat_map(|source| {
                source
                    .profiles()
                    .iter()
                    .flat_map(|p| p.roots.iter().cloned())
            })
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    /// Whether some agent owns `path`: the filter for raw watcher events.
    #[must_use]
    pub fn owns(&self, path: &Utf8Path) -> bool {
        self.settings
            .sources
            .iter()
            .any(|source| source.owns(path) && source.profiles().iter().any(|p| p.contains(path)))
    }

    /// Runs one scan and returns what it did. Only a store failure ends it early; a file that
    /// cannot be read or parsed becomes a parse issue and the other files go on.
    pub fn scan(
        &mut self,
        store: &mut Store,
        request: &ScanRequest,
    ) -> Result<ScanReport, IngestError> {
        let wall = Instant::now();
        let now = (self.clock)();
        apply_priority(request.priority);
        self.pricer.refresh(store, now)?;
        let redaction = self
            .settings
            .redact_paths
            .then(|| store.redaction_key().clone());
        let mut cache = ProjectCache::load(store, redaction)?;
        let scope = match request.mode {
            ScanMode::Full => Reresolve::All,
            ScanMode::Incremental if !self.fallbacks_retried => Reresolve::StaleAndFallbacks,
            ScanMode::Incremental => Reresolve::Stale,
        };
        let agents: Vec<AgentId> = self.settings.sources.iter().map(AgentSource::id).collect();
        let (cwds_reresolved, events_reassigned) =
            cache.reresolve(store, &self.resolver, scope, &agents, now)?;
        self.fallbacks_retried = true;

        let mut states: Vec<AgentState> = self
            .settings
            .sources
            .iter()
            .map(|source| AgentState::new(source.id(), selected(request, source.id()), now))
            .collect();
        let mut tasks = self.plan(store, request, now, &mut states)?;
        tasks.sort_by_key(|task| std::cmp::Reverse(task.remaining()));

        let machine = store.machine_id();
        let mut run = Run {
            store,
            reader: &self.reader,
            pricer: &self.pricer,
            resolver: &self.resolver,
            cache,
            machine,
            now,
            tasks: &tasks,
            states,
            #[cfg(test)]
            crash_before_commit: self.crash_before_commit,
        };
        self.parse_all(&mut run, request.priority)?;
        for slot in 0..run.states.len() {
            if run.states[slot].selected {
                run.flush(slot)?;
            }
        }

        let Run {
            store, mut states, ..
        } = run;
        for (slot, source) in self.settings.sources.iter().enumerate() {
            let state = &mut states[slot];
            if !state.selected {
                continue;
            }
            let agent = state.agent;
            match state.scan.drift.threshold_issue() {
                Some(issue) => {
                    store.record_parse_issue(&to_stored(agent, SCAN_ISSUE_SUBJECT, &issue, now))?;
                }
                None if state.scan.drift.usage_lines > 0 => {
                    store.close_parse_issue(
                        agent,
                        IssueKind::FormatDrift,
                        SCAN_ISSUE_SUBJECT,
                        now,
                    )?;
                }
                None => {}
            }
            if self.settings.import_claude_stats_cache
                && request.paths.is_none()
                && source.id() == AgentId::CLAUDE
            {
                let scan = self.import_aggregates(store, source, now)?;
                state.scan.aggregates = Some(scan);
            }
            if state.scan.files_parsed > 0 {
                store.record_ingest_run(&state.ingest_run((self.clock)()))?;
            }
        }
        Ok(ScanReport {
            agents: states
                .into_iter()
                .filter(|state| state.selected)
                .map(|state| state.scan)
                .collect(),
            cwds_reresolved,
            events_reassigned,
            elapsed: wall.elapsed(),
        })
    }

    /// Compares every candidate file with its checkpoint and returns the parse tasks. New files
    /// get a placeholder checkpoint so their events can carry a source-file id (D45).
    fn plan(
        &self,
        store: &mut Store,
        request: &ScanRequest,
        now: Timestamp,
        states: &mut [AgentState],
    ) -> Result<Vec<Task>, IngestError> {
        let mut tasks = Vec::new();
        for (slot, source) in self.settings.sources.iter().enumerate() {
            if !states[slot].selected {
                continue;
            }
            let agent = source.id();
            let known: HashMap<Utf8PathBuf, StoredCheckpoint> = store
                .load_checkpoints(agent)?
                .into_iter()
                .map(|stored| (stored.checkpoint.path.clone(), stored))
                .collect();
            let files = candidates(store, source, request, now)?;
            let scan = &mut states[slot].scan;
            for mut file in files {
                let stat = match FileStat::of(&file.path) {
                    Ok(stat) => stat,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(err) => {
                        record_file_problem(store, agent, &file.path, &err.to_string(), now)?;
                        scan.files_discovered += 1;
                        scan.files_failed += 1;
                        continue;
                    }
                };
                scan.files_discovered += 1;
                let stored = known.get(&file.path);
                let plan = if request.mode == ScanMode::Full {
                    Ok(Plan::Parse {
                        from: 0,
                        reason: ParseReason::Forced,
                    })
                } else {
                    decide(stored.map(|s| &s.checkpoint), &stat, |offset| {
                        last_line_hash(&file.path, offset)
                    })
                };
                let (from, reason) = match plan {
                    Ok(Plan::Skip) => {
                        scan.files_unchanged += 1;
                        continue;
                    }
                    Ok(Plan::Parse { from, reason }) => (from, reason),
                    Err(err) => {
                        record_file_problem(store, agent, &file.path, &err.to_string(), now)?;
                        scan.files_failed += 1;
                        continue;
                    }
                };
                let id = match stored {
                    Some(stored) => stored.id,
                    None => store.save_checkpoint(&Checkpoint {
                        path: file.path.clone(),
                        agent,
                        profile: file.profile.clone(),
                        dev: stat.dev,
                        inode: stat.inode,
                        size: 0,
                        mtime: Timestamp::UNIX_EPOCH,
                        offset: 0,
                        last_line_hash: None,
                    })?,
                };
                file.id = Some(id);
                tasks.push(Task {
                    slot,
                    file,
                    stat,
                    from,
                    reason,
                    expected: stored.and_then(|stored| stored.checkpoint.last_line_hash),
                });
            }
        }
        Ok(tasks)
    }

    /// The parse pool, built once per ingestor: one thread per core at user-initiated priority.
    fn pool(&self) -> Result<&rayon::ThreadPool, IngestError> {
        if let Some(pool) = self.pool.get() {
            return Ok(pool);
        }
        let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("tokenmtr-parse-{index}"))
            .start_handler(|_| {
                if let Err(err) = set_thread_priority(ThreadPriority::UserInitiated) {
                    tracing::debug!(%err, "cannot raise parse thread priority");
                }
            })
            .build()?;
        Ok(self.pool.get_or_init(|| pool))
    }

    /// Parses every task and hands the results to the writer. A user-initiated scan with more
    /// than one file fans the parsing out over a rayon pool; results come back through a bounded
    /// channel, so memory stays bounded while the writer — this thread — is the only one
    /// touching the store (D48).
    fn parse_all(&self, run: &mut Run<'_>, priority: ScanPriority) -> Result<(), IngestError> {
        let tasks = run.tasks;
        let sources = &self.settings.sources;
        let parallel = priority == ScanPriority::UserInitiated && tasks.len() > 1;
        if !parallel {
            for (index, task) in tasks.iter().enumerate() {
                run.handle(parse_task(&sources[task.slot], task, index))?;
            }
            return Ok(());
        }
        let pool = self.pool()?;
        let threads = pool.current_num_threads().min(tasks.len());
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::sync_channel::<Parsed>(threads);
            let producer = scope.spawn(|| {
                pool.install(|| {
                    (0..tasks.len())
                        .into_par_iter()
                        .with_max_len(1)
                        .for_each_with(tx, |tx, index| {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let task = &tasks[index];
                            // A closed channel means the writer failed; the scan is ending.
                            let _ = tx.send(parse_task(&sources[task.slot], task, index));
                        });
                });
            });
            let mut outcome = Ok(());
            for parsed in &rx {
                if let Err(err) = run.handle(parsed) {
                    cancel.store(true, Ordering::Relaxed);
                    outcome = Err(err);
                    break;
                }
            }
            drop(rx);
            if producer.join().is_err() {
                return Err(IngestError::Corrupt("a parse worker panicked".to_owned()));
            }
            outcome
        })
    }

    /// Imports Claude's stats cache for the days before the first transcript event (D38).
    fn import_aggregates(
        &self,
        store: &mut Store,
        source: &AgentKind,
        now: Timestamp,
    ) -> Result<AggregateScan, IngestError> {
        let agent = source.id();
        let machine = store.machine_id();
        let mut summary = AggregateScan::default();
        for profile in source.profiles() {
            let found = match source.aggregates(profile, &self.settings.zone) {
                Ok(found) => found,
                Err(err) => {
                    // Each profile's own subject, so one profile's failure does not overwrite
                    // another's.
                    let subject = match &err {
                        crate::AgentError::Io { path, .. } => path.clone(),
                        crate::AgentError::Id(_) => {
                            Utf8PathBuf::from(format!("stats-cache of profile {}", profile.id))
                        }
                    };
                    record_file_problem(store, agent, &subject, &err.to_string(), now)?;
                    continue;
                }
            };
            for (path, issue) in &found.issues {
                let issue = to_stored(agent, path.as_str(), issue, now);
                store.record_parse_issue(&issue)?;
            }
            for path in &found.clean {
                for kind in [IssueKind::FormatDrift, IssueKind::ParseError] {
                    store.close_parse_issue(agent, kind, path.as_str(), now)?;
                }
            }
            let cutoff = self
                .reader
                .earliest_transcript_event(agent, &profile.id)?
                .map(|at| at.to_zoned(self.settings.zone.clone()).date());
            let mut rows: Vec<UsageEvent> = Vec::new();
            for mut event in found.events {
                summary.found += 1;
                let day = event.at.to_zoned(self.settings.zone.clone()).date();
                if cutoff.is_some_and(|first| day >= first) {
                    summary.covered_by_transcripts += 1;
                    continue;
                }
                let estimate = self.pricer.price(&self.reader, &mut event)?;
                rows.push(event.into_usage_event(&machine, estimate));
            }
            for chunk in rows.chunks(CHUNK) {
                summary.written.add(&store.upsert_events(chunk)?);
            }
            store.supersede_daily_aggregates_covered_by_transcripts(
                agent,
                &profile.id,
                &self.settings.zone,
            )?;
        }
        Ok(summary)
    }
}

fn selected(request: &ScanRequest, agent: AgentId) -> bool {
    request
        .agents
        .as_ref()
        .is_none_or(|agents| agents.contains(&agent))
}

fn apply_priority(priority: ScanPriority) {
    let class = match priority {
        ScanPriority::UserInitiated => ThreadPriority::UserInitiated,
        ScanPriority::Background => ThreadPriority::Background,
    };
    if let Err(err) = set_thread_priority(class) {
        tracing::debug!(%err, "cannot set the scan thread priority");
    }
}

/// The files a scan considers for `source`.
fn candidates(
    store: &mut Store,
    source: &AgentKind,
    request: &ScanRequest,
    now: Timestamp,
) -> Result<Vec<SourceFile>, IngestError> {
    let agent = source.id();
    if let Some(paths) = &request.paths {
        let mut files: Vec<SourceFile> = paths
            .iter()
            .filter(|path| source.owns(path))
            .filter_map(|path| {
                source
                    .profiles()
                    .iter()
                    .find(|profile| profile.contains(path))
                    .map(|profile| SourceFile::new(path.clone(), profile.id.clone()))
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path);
        return Ok(files);
    }
    let found = discover(source);
    for issue in &found.issues {
        store.record_parse_issue(&to_stored(agent, "discovery", issue, now))?;
    }
    Ok(found.files)
}

fn record_file_problem(
    store: &mut Store,
    agent: AgentId,
    path: &Utf8Path,
    reason: &str,
    at: Timestamp,
) -> Result<(), IngestError> {
    store.record_parse_issue(&StoredIssue {
        agent,
        kind: IssueKind::ParseError,
        subject: path.as_str().to_owned(),
        count: std::num::NonZeroU64::MIN,
        sample: Some(reason.to_owned()),
        at,
    })?;
    Ok(())
}

/// Maps an agent-side issue to the stored shape. `subject` is the file the issue was found in;
/// issues on the same `(agent, kind, subject)` merge in the store (D42).
fn to_stored(agent: AgentId, subject: &str, issue: &ParseIssue, at: Timestamp) -> StoredIssue {
    let (kind, subject, count, sample) = match issue {
        ParseIssue::FormatDrift {
            reason,
            lines,
            sample,
        } => {
            let reason = reason_text(*reason);
            let sample = match sample {
                Some(skeleton) => format!("{reason}: {}", skeleton.as_str()),
                None => reason,
            };
            (
                IssueKind::FormatDrift,
                subject.to_owned(),
                *lines,
                Some(sample),
            )
        }
        ParseIssue::UnknownVersion { version } => (
            IssueKind::UnknownVersion,
            subject.to_owned(),
            1,
            Some(version.clone()),
        ),
        ParseIssue::Unreadable { path, reason } => (
            IssueKind::ParseError,
            path.as_str().to_owned(),
            1,
            Some(reason.clone()),
        ),
    };
    StoredIssue {
        agent,
        kind,
        subject,
        count: std::num::NonZeroU64::new(count).unwrap_or(std::num::NonZeroU64::MIN),
        sample,
        at,
    }
}

fn reason_text(reason: DriftReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "format_drift".to_owned())
}

fn count_of(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// One file to parse.
struct Task {
    slot: usize,
    file: SourceFile,
    stat: FileStat,
    from: u64,
    reason: ParseReason,
    /// The stored last-line hash at `from`, which the bytes before `from` must still match.
    expected: Option<[u8; 32]>,
}

impl Task {
    fn remaining(&self) -> u64 {
        self.stat.size.saturating_sub(self.from)
    }

    /// Where the parse starts. Planning checked the prefix, but the file may have been rewritten
    /// in place since (pi's `_rewriteFile` truncates and rewrites the same inode): a prefix that
    /// no longer ends with the stored line, newline included, restarts the file from zero.
    /// Event dedup keys make the re-read idempotent.
    fn start(&self) -> (u64, ParseReason) {
        if self.from == 0 {
            return (0, self.reason);
        }
        match last_line_hash(&self.file.path, self.from) {
            Ok(Some(hash)) if Some(hash) == self.expected => (self.from, self.reason),
            _ => (0, ParseReason::Rewritten),
        }
    }
}

/// What a worker hands to the writer.
struct Parsed {
    task: usize,
    elapsed: Duration,
    result: Result<ParsedOk, String>,
}

struct ParsedOk {
    parts: BatchParts,
    outcome: ParseOutcome,
    /// Where the parse actually started, and why ([`Task::start`]).
    from: u64,
    reason: ParseReason,
}

/// Parses one file, converting a panic into an error so one bad file cannot end the scan.
fn parse_task(source: &AgentKind, task: &Task, index: usize) -> Parsed {
    let started = Instant::now();
    let (from, reason) = task.start();
    let parsed = catch_unwind(AssertUnwindSafe(|| {
        let mut batch = ParseBatch::new();
        source
            .parse(&task.file, ByteOffset::new(from), &mut batch)
            .map(|outcome| (batch.into_parts(), outcome))
    }));
    let result = match parsed {
        Ok(Ok((parts, outcome))) => Ok(ParsedOk {
            parts,
            outcome,
            from,
            reason,
        }),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_text(payload.as_ref())),
    };
    Parsed {
        task: index,
        elapsed: started.elapsed(),
        result,
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    format!("parser panicked: {text}")
}

/// The offset and last-line hash to store after a parse that started at `from`.
///
/// The hash is that of the bytes parsed; the file is read only when no line ended there. An
/// unterminated tail older than [`ABANDONED_TAIL`] is a crashed writer's: it is skipped (and
/// counted as malformed) so the next line the agent appends parses alone.
fn checkpoint_end(
    task: &Task,
    from: u64,
    outcome: &mut ParseOutcome,
    now: Timestamp,
) -> (u64, Option<[u8; 32]>) {
    let to = outcome.new_offset.get();
    if to < task.stat.size && now.duration_since(task.stat.mtime) > ABANDONED_TAIL {
        outcome.drift.skipped.malformed += 1;
        let size = task.stat.size;
        return (size, last_line_hash(&task.file.path, size).unwrap_or(None));
    }
    let hash = match outcome.last_line_hash {
        Some(hash) if to > from => Some(hash),
        _ => last_line_hash(&task.file.path, to).unwrap_or(None),
    };
    (to, hash)
}

/// Who an event belongs to: the fields a forked copy must not overwrite.
#[derive(Clone)]
struct Owner {
    session: SessionId,
    parent_session: Option<SessionId>,
    cwd: Option<Utf8PathBuf>,
    git_branch: Option<String>,
    source_file: Option<SourceFileId>,
}

impl Owner {
    fn of_parsed(event: &ParsedEvent) -> Self {
        Self {
            session: event.session.clone(),
            parent_session: event.parent_session.clone(),
            cwd: event.cwd.clone(),
            git_branch: event.git_branch.clone(),
            source_file: event.source_file,
        }
    }

    fn of_stored(event: UsageEvent) -> Self {
        Self {
            session: event.session,
            parent_session: event.parent_session,
            cwd: event.cwd,
            git_branch: event.git_branch,
            source_file: event.source_file,
        }
    }

    fn apply(self, event: &mut ParsedEvent) {
        event.session = self.session;
        event.parent_session = self.parent_session;
        event.cwd = self.cwd;
        event.git_branch = self.git_branch;
        event.source_file = self.source_file;
    }
}

#[derive(Default)]
struct Pending {
    events: Vec<UsageEvent>,
    /// Attribution of the pending events by dedup key, for [`Run::keep_owner`]; flushed events
    /// are found in the store.
    owners: HashMap<DedupKey, Owner>,
    calls: Vec<ProfiledToolCall>,
    signals: Vec<SessionSignal>,
    orphans: Vec<ToolResultFlag>,
    issues: Vec<IssueUpdate>,
    checkpoints: Vec<Checkpoint>,
}

struct AgentState {
    agent: AgentId,
    selected: bool,
    started: Timestamp,
    scan: AgentScan,
    pending: Pending,
}

impl AgentState {
    fn new(agent: AgentId, selected: bool, started: Timestamp) -> Self {
        Self {
            agent,
            selected,
            started,
            scan: AgentScan::new(agent),
            pending: Pending::default(),
        }
    }

    fn ingest_run(&self, finished: Timestamp) -> IngestRun {
        IngestRun {
            agent: self.agent,
            started: self.started,
            finished,
            files_parsed: self.scan.files_parsed,
            bytes_parsed: self.scan.bytes_parsed,
            lines_read: self.scan.drift.lines_read,
            events_found: self.scan.drift.events,
            events_changed: self.scan.events.changed(),
            skipped: self.scan.skipped_counters(),
        }
    }
}

/// The single writer of one scan.
struct Run<'a> {
    store: &'a mut Store,
    reader: &'a StoreReader,
    pricer: &'a Pricer,
    resolver: &'a ProjectResolver,
    cache: ProjectCache,
    machine: MachineId,
    now: Timestamp,
    tasks: &'a [Task],
    states: Vec<AgentState>,
    #[cfg(test)]
    crash_before_commit: bool,
}

impl Run<'_> {
    fn handle(&mut self, parsed: Parsed) -> Result<(), IngestError> {
        let tasks = self.tasks;
        let task = &tasks[parsed.task];
        self.states[task.slot].scan.parse_time += parsed.elapsed;
        match parsed.result {
            Ok(ok) => self.accept(task, ok),
            Err(message) => {
                self.reject(task, &message);
                Ok(())
            }
        }
    }

    fn reject(&mut self, task: &Task, message: &str) {
        let state = &mut self.states[task.slot];
        state.scan.files_failed += 1;
        state.scan.files.push(FileScan {
            path: task.file.path.clone(),
            reason: task.reason,
            from: task.from,
            to: task.from,
            events: 0,
            error: Some(message.to_owned()),
        });
        state.pending.issues.push(IssueUpdate::Record(StoredIssue {
            agent: state.agent,
            kind: IssueKind::ParseError,
            subject: task.file.path.as_str().to_owned(),
            count: std::num::NonZeroU64::MIN,
            sample: Some(message.to_owned()),
            at: self.now,
        }));
    }

    /// Prices, attributes and stages the events of one parsed file, flushing every `CHUNK`.
    fn stage_events(&mut self, task: &Task, events: Vec<ParsedEvent>) -> Result<(), IngestError> {
        let slot = task.slot;
        let agent = self.states[slot].agent;
        for mut parsed in events {
            parsed.source_file = task.file.id;
            if parsed.parent_session.is_some() {
                self.keep_owner(slot, &mut parsed)?;
            }
            self.states[slot]
                .pending
                .owners
                .insert(parsed.dedup_key.clone(), Owner::of_parsed(&parsed));
            let assignment = match &parsed.cwd {
                Some(cwd) => self.cache.assign(
                    self.store,
                    self.resolver,
                    agent,
                    cwd,
                    self.now,
                    &mut self.states[slot].pending.issues,
                )?,
                None => None,
            };
            let estimate = self.pricer.price(self.reader, &mut parsed)?;
            let mut event = parsed.into_usage_event(&self.machine, estimate);
            self.cache.label(&mut event, assignment);
            self.states[slot].pending.events.push(event);
            if self.states[slot].pending.events.len() >= CHUNK {
                self.flush(slot)?;
            }
        }
        Ok(())
    }

    fn accept(&mut self, task: &Task, ok: ParsedOk) -> Result<(), IngestError> {
        let ParsedOk {
            parts,
            mut outcome,
            from,
            reason,
        } = ok;
        let slot = task.slot;
        let agent = self.states[slot].agent;
        let (to, hash) = checkpoint_end(task, from, &mut outcome, self.now);
        let events_found = count_of(parts.events.len());
        self.stage_events(task, parts.events)?;

        let machine = &self.machine;
        let state = &mut self.states[slot];
        let pending = &mut state.pending;
        pending
            .calls
            .extend(parts.tool_calls.into_iter().map(|call| ProfiledToolCall {
                profile: task.file.profile.clone(),
                call: call.into_tool_call(machine),
            }));
        pending
            .signals
            .extend(parts.signals.into_iter().map(|signal| SessionSignal {
                agent: signal.agent,
                session: signal.session,
                kind: signal.kind,
                at: signal.at,
            }));
        pending
            .orphans
            .extend(
                parts
                    .orphan_tool_results
                    .into_iter()
                    .map(|result| ToolResultFlag {
                        agent: result.agent,
                        tool_use_id: result.tool_use_id,
                        is_error: result.is_error,
                    }),
            );

        let subject = task.file.path.as_str();
        let mut recorded = Vec::new();
        for issue in &outcome.issues {
            let stored = to_stored(agent, subject, issue, self.now);
            recorded.push(stored.kind);
            pending.issues.push(IssueUpdate::Record(stored));
        }
        // A parse that read the file from the start and found no problem of a kind settles the
        // earlier issue of that kind; an appended parse says nothing about earlier lines.
        let mut settles = vec![IssueKind::ParseError];
        if from == 0 {
            settles.extend([IssueKind::FormatDrift, IssueKind::UnknownVersion]);
        }
        for kind in settles.into_iter().filter(|kind| !recorded.contains(kind)) {
            pending.issues.push(IssueUpdate::Close {
                agent,
                kind,
                subject: subject.to_owned(),
                at: self.now,
            });
        }

        pending.checkpoints.push(Checkpoint {
            path: task.file.path.clone(),
            agent,
            profile: task.file.profile.clone(),
            dev: task.stat.dev,
            inode: task.stat.inode,
            size: task.stat.size.max(to),
            mtime: task.stat.mtime,
            offset: to,
            last_line_hash: hash,
        });

        let scan = &mut state.scan;
        scan.files_parsed += 1;
        scan.bytes_parsed += to.saturating_sub(from);
        scan.drift.merge(&outcome.drift);
        scan.files.push(FileScan {
            path: task.file.path.clone(),
            reason,
            from,
            to,
            events: events_found,
            error: None,
        });
        Ok(())
    }

    /// Keeps the attribution of an event a forked session copied (M6): when its key already
    /// belongs to another session, pending or stored, the copy takes that session's attribution
    /// and only refreshes the usage. Without this the store would move the event to whichever
    /// file the parse pool finished last. A fork's own events have keys nobody else holds.
    fn keep_owner(&self, slot: usize, parsed: &mut ParsedEvent) -> Result<(), IngestError> {
        let owner = match self.states[slot].pending.owners.get(&parsed.dedup_key) {
            Some(owner) => Some(owner.clone()),
            None => self
                .reader
                .usage_event_by_key(parsed.agent, &parsed.dedup_key)?
                .map(|stored| Owner::of_stored(stored.row)),
        };
        if let Some(owner) = owner.filter(|owner| owner.session != parsed.session) {
            owner.apply(parsed);
        }
        Ok(())
    }

    /// Writes everything pending for `slot` in one transaction: events (at most `CHUNK`),
    /// tool calls, signals, tool results of earlier scans, issue changes and the checkpoints. A
    /// checkpoint therefore never runs ahead of its rows, and a crash stores nothing.
    fn flush(&mut self, slot: usize) -> Result<(), IngestError> {
        let pending = std::mem::take(&mut self.states[slot].pending);
        #[cfg(test)]
        if self.crash_before_commit {
            return Err(IngestError::Crash);
        }
        let written = self.store.commit_ingest(&IngestWrite {
            events: &pending.events,
            tool_calls: &pending.calls,
            signals: &pending.signals,
            tool_results: &pending.orphans,
            issues: &pending.issues,
            checkpoints: &pending.checkpoints,
        })?;
        let scan = &mut self.states[slot].scan;
        scan.events.add(&written.events);
        scan.tool_calls.add(&written.tool_calls);
        scan.signals_new += written.signals_new;
        scan.orphan_results_applied += written.tool_results_applied;
        scan.orphan_results_unmatched += written.tool_results_unmatched;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tokenmtr_core::ProfileId;

    use super::*;
    use crate::testkit::utf8_dir;

    /// The file changed between planning and parsing (pi rewrites a session in place): the
    /// parse must not resume at the planned offset.
    #[test]
    fn a_prefix_rewritten_after_planning_restarts_the_parse_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = utf8_dir(&dir).join("s.jsonl");
        fs::write(&path, "first\nsecond\n").unwrap();
        let expected = last_line_hash(&path, 13).unwrap();
        let task = |from| Task {
            slot: 0,
            file: SourceFile::new(path.clone(), ProfileId::default_profile()),
            stat: FileStat::of(&path).unwrap(),
            from,
            reason: ParseReason::Appended,
            expected,
        };
        assert_eq!(task(13).start(), (13, ParseReason::Appended));
        assert_eq!(task(0).start(), (0, ParseReason::Appended));

        fs::write(&path, "first\nsecund\nthird\n").unwrap();
        assert_eq!(task(13).start(), (0, ParseReason::Rewritten));
        fs::write(&path, "xsecond\nsecond\n").unwrap();
        assert_eq!(
            task(13).start(),
            (0, ParseReason::Rewritten),
            "13 is no longer a line end"
        );
    }
}
