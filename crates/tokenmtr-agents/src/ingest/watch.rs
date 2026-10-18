//! File watching for the daemon (D16): raw `notify` events, our own debounce, a startup rescan.
//!
//! [`WatchCore`] holds all decisions and takes explicit instants, so tests drive it without a
//! clock or a file system. [`Watch`] is the thin OS part: it feeds `notify` events into a core
//! and waits for the next [`WatchBatch`]. A missing root is skipped; the periodic sweep
//! (`daemon.scan_interval`) picks up files in roots that appear later.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use super::error::IngestError;

/// A path is scanned this long after its last event (D16).
pub const QUIET: Duration = Duration::from_secs(2);

/// A path that keeps changing is scanned at least this long after its first pending event, so a
/// transcript written continuously during a long agent turn still reaches the store.
pub const MAX_WAIT: Duration = Duration::from_secs(10);

/// Debounce timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Debounce {
    /// Quiet time after the last event of a path before it is due.
    pub quiet: Duration,
    /// Longest a path with pending events waits.
    pub max_wait: Duration,
}

impl Default for Debounce {
    fn default() -> Self {
        Self {
            quiet: QUIET,
            max_wait: MAX_WAIT,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    first: Instant,
    last: Instant,
}

/// Per-path trailing debounce.
#[derive(Debug, Default)]
pub struct Debouncer {
    timing: Debounce,
    pending: HashMap<Utf8PathBuf, Pending>,
}

impl Debouncer {
    /// A debouncer with `timing`.
    #[must_use]
    pub fn new(timing: Debounce) -> Self {
        Self {
            timing,
            pending: HashMap::new(),
        }
    }

    /// Notes an event for `path` at `now`.
    pub fn note(&mut self, path: Utf8PathBuf, now: Instant) {
        self.pending
            .entry(path)
            .and_modify(|p| p.last = now)
            .or_insert(Pending {
                first: now,
                last: now,
            });
    }

    fn due_at(&self, pending: &Pending) -> Instant {
        (pending.last + self.timing.quiet).min(pending.first + self.timing.max_wait)
    }

    /// Removes and returns, sorted, the paths due at `now`.
    pub fn take_due(&mut self, now: Instant) -> Vec<Utf8PathBuf> {
        let mut due: Vec<Utf8PathBuf> = self
            .pending
            .iter()
            .filter(|(_, p)| self.due_at(p) <= now)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &due {
            self.pending.remove(path);
        }
        due.sort();
        due
    }

    /// When the first pending path is due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| self.due_at(p)).min()
    }

    /// Forgets every pending path.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

/// What the daemon should scan next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchBatch {
    /// Scan everything: at startup, and after the OS dropped events.
    Rescan,
    /// Scan only these changed files.
    Paths(Vec<Utf8PathBuf>),
}

/// A raw event reduced to what the core needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawEvent {
    /// Files that were created, changed or removed.
    Changed(Vec<Utf8PathBuf>),
    /// The OS lost events, or the watcher failed: only a full scan is safe.
    Rescan,
}

impl RawEvent {
    /// Reduces a `notify` event. Pure access events (reads) are ignored.
    #[must_use]
    pub fn from_notify(event: &notify::Event) -> Option<Self> {
        if event.need_rescan() {
            return Some(Self::Rescan);
        }
        match event.kind {
            EventKind::Access(_) => None,
            EventKind::Any
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Remove(_)
            | EventKind::Other => Some(Self::Changed(
                event
                    .paths
                    .iter()
                    .filter_map(|p| Utf8Path::from_path(p).map(Utf8Path::to_owned))
                    .collect(),
            )),
        }
    }
}

/// The decisions of the watcher, independent of the OS.
#[derive(Debug)]
pub struct WatchCore {
    debouncer: Debouncer,
    rescan: bool,
}

impl WatchCore {
    /// A core that starts with a pending [`WatchBatch::Rescan`]: the startup rescan (D16).
    #[must_use]
    pub fn new(timing: Debounce) -> Self {
        Self {
            debouncer: Debouncer::new(timing),
            rescan: true,
        }
    }

    /// Feeds one raw event. Paths `owns` rejects are dropped.
    pub fn feed(&mut self, event: RawEvent, now: Instant, owns: &dyn Fn(&Utf8Path) -> bool) {
        match event {
            RawEvent::Rescan => self.rescan = true,
            RawEvent::Changed(paths) => {
                for path in paths.into_iter().filter(|p| owns(p)) {
                    self.debouncer.note(path, now);
                }
            }
        }
    }

    /// The batch to scan at `now`, if any. A pending rescan wins and covers the pending paths.
    pub fn poll(&mut self, now: Instant) -> Option<WatchBatch> {
        if self.rescan {
            self.rescan = false;
            self.debouncer.clear();
            return Some(WatchBatch::Rescan);
        }
        let due = self.debouncer.take_due(now);
        (!due.is_empty()).then_some(WatchBatch::Paths(due))
    }

    /// How long the caller may sleep before [`poll`](Self::poll) can return something.
    #[must_use]
    pub fn sleep_hint(&self, now: Instant) -> Option<Duration> {
        if self.rescan {
            return Some(Duration::ZERO);
        }
        self.debouncer
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

/// A recursive watch on the agent roots.
pub struct Watch {
    // Dropping the watcher stops the events.
    _watcher: RecommendedWatcher,
    events: mpsc::Receiver<notify::Result<notify::Event>>,
    core: WatchCore,
    missing: Vec<Utf8PathBuf>,
}

impl Watch {
    /// Watches every existing root recursively with the default debounce.
    pub fn start(roots: &[Utf8PathBuf]) -> Result<Self, IngestError> {
        Self::start_with(roots, Debounce::default())
    }

    /// Like [`start`](Self::start) with explicit timing.
    pub fn start_with(roots: &[Utf8PathBuf], timing: Debounce) -> Result<Self, IngestError> {
        let (tx, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            // The receiver is gone only when the watch is being dropped.
            let _ = tx.send(event);
        })?;
        let mut missing = Vec::new();
        for root in roots {
            if root.is_dir() {
                watcher.watch(root.as_std_path(), RecursiveMode::Recursive)?;
            } else {
                missing.push(root.clone());
            }
        }
        Ok(Self {
            _watcher: watcher,
            events,
            core: WatchCore::new(timing),
            missing,
        })
    }

    /// Roots that did not exist at start. Only the periodic sweep sees files in them.
    #[must_use]
    pub fn missing_roots(&self) -> &[Utf8PathBuf] {
        &self.missing
    }

    /// Blocks up to `timeout` for the next batch. The first batch is [`WatchBatch::Rescan`].
    /// A watcher error asks for a rescan too: after an error events may have been lost.
    pub fn next_batch(
        &mut self,
        timeout: Duration,
        owns: &dyn Fn(&Utf8Path) -> bool,
    ) -> Option<WatchBatch> {
        let give_up = Instant::now() + timeout;
        loop {
            while let Ok(event) = self.events.try_recv() {
                self.feed(event, owns);
            }
            let now = Instant::now();
            if let Some(batch) = self.core.poll(now) {
                return Some(batch);
            }
            let remaining = give_up.saturating_duration_since(now);
            if remaining.is_zero() {
                return None;
            }
            let wait = self
                .core
                .sleep_hint(now)
                .map_or(remaining, |hint| hint.min(remaining));
            match self.events.recv_timeout(wait) {
                Ok(event) => self.feed(event, owns),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    fn feed(&mut self, event: notify::Result<notify::Event>, owns: &dyn Fn(&Utf8Path) -> bool) {
        let raw = match event {
            Ok(event) => RawEvent::from_notify(&event),
            Err(err) => {
                tracing::warn!(%err, "file watcher error, scheduling a rescan");
                Some(RawEvent::Rescan)
            }
        };
        if let Some(raw) = raw {
            self.core.feed(raw, Instant::now(), owns);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn timing() -> Debounce {
        Debounce {
            quiet: Duration::from_millis(2000),
            max_wait: Duration::from_millis(10_000),
        }
    }

    fn path(name: &str) -> Utf8PathBuf {
        Utf8PathBuf::from(name)
    }

    fn all(_: &Utf8Path) -> bool {
        true
    }

    #[test]
    fn a_path_is_due_after_the_quiet_time_and_only_once() {
        let base = Instant::now();
        let mut d = Debouncer::new(timing());
        d.note(path("/a"), base);
        assert_eq!(d.take_due(at(base, 1999)), Vec::<Utf8PathBuf>::new());
        assert_eq!(d.take_due(at(base, 2000)), [path("/a")]);
        assert_eq!(d.take_due(at(base, 9999)), Vec::<Utf8PathBuf>::new());
    }

    #[test]
    fn each_event_restarts_the_quiet_time_of_its_path_only() {
        let base = Instant::now();
        let mut d = Debouncer::new(timing());
        d.note(path("/a"), base);
        d.note(path("/b"), at(base, 1500));
        d.note(path("/a"), at(base, 1500));
        assert_eq!(d.take_due(at(base, 2000)), Vec::<Utf8PathBuf>::new());
        assert_eq!(d.take_due(at(base, 3500)), [path("/a"), path("/b")]);
    }

    #[test]
    fn a_busy_path_is_still_scanned_after_the_max_wait() {
        let base = Instant::now();
        let mut d = Debouncer::new(timing());
        for step in 0..=12 {
            d.note(path("/busy"), at(base, step * 1000));
        }
        assert_eq!(d.take_due(at(base, 12_000)), [path("/busy")]);
    }

    #[test]
    fn the_deadline_is_the_earliest_due_time() {
        let base = Instant::now();
        let mut d = Debouncer::new(timing());
        assert_eq!(d.next_deadline(), None);
        d.note(path("/a"), at(base, 100));
        d.note(path("/b"), at(base, 50));
        assert_eq!(d.next_deadline(), Some(at(base, 2050)));
    }

    #[test]
    fn the_first_batch_is_a_rescan_and_covers_pending_paths() {
        let base = Instant::now();
        let mut core = WatchCore::new(timing());
        assert_eq!(core.sleep_hint(base), Some(Duration::ZERO));
        core.feed(RawEvent::Changed(vec![path("/a")]), base, &all);
        assert_eq!(core.poll(base), Some(WatchBatch::Rescan));
        assert_eq!(core.poll(at(base, 5000)), None, "the rescan covered /a");
    }

    #[test]
    fn changed_paths_become_a_batch_after_the_quiet_time() {
        let base = Instant::now();
        let mut core = WatchCore::new(timing());
        let _ = core.poll(base);
        core.feed(
            RawEvent::Changed(vec![path("/a.jsonl"), path("/note.txt")]),
            base,
            &|p| p.extension() == Some("jsonl"),
        );
        assert_eq!(core.poll(at(base, 1000)), None);
        assert_eq!(
            core.sleep_hint(at(base, 1000)),
            Some(Duration::from_millis(1000))
        );
        assert_eq!(
            core.poll(at(base, 2000)),
            Some(WatchBatch::Paths(vec![path("/a.jsonl")]))
        );
    }

    #[test]
    fn lost_events_ask_for_a_rescan() {
        let base = Instant::now();
        let mut core = WatchCore::new(timing());
        let _ = core.poll(base);
        core.feed(RawEvent::Rescan, base, &all);
        assert_eq!(core.poll(base), Some(WatchBatch::Rescan));
    }

    #[test]
    fn reads_are_not_changes() {
        let read = notify::Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .add_path("/a".into());
        assert_eq!(RawEvent::from_notify(&read), None);
        let write = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path("/a".into());
        assert_eq!(
            RawEvent::from_notify(&write),
            Some(RawEvent::Changed(vec![path("/a")]))
        );
        let lost = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert_eq!(RawEvent::from_notify(&lost), Some(RawEvent::Rescan));
    }

    #[test]
    fn the_os_watcher_reports_a_rescan_then_the_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::testkit::utf8_dir(&dir);
        let mut watch = Watch::start_with(
            std::slice::from_ref(&root),
            Debounce {
                quiet: Duration::from_millis(50),
                max_wait: Duration::from_millis(500),
            },
        )
        .unwrap();
        let owns = |p: &Utf8Path| p.extension() == Some("jsonl");
        assert_eq!(
            watch.next_batch(Duration::from_secs(5), &owns),
            Some(WatchBatch::Rescan)
        );
        let file = root.join("s.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = false;
        while Instant::now() < deadline && !seen {
            if let Some(WatchBatch::Paths(paths)) = watch.next_batch(Duration::from_secs(1), &owns)
            {
                seen = paths.iter().any(|p| p.file_name() == Some("s.jsonl"));
            }
        }
        assert!(seen, "no event for {file}");
        assert_eq!(watch.missing_roots().len(), 0);
    }

    #[test]
    fn a_missing_root_is_skipped_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        let gone = crate::testkit::utf8_dir(&dir).join("absent");
        let watch = Watch::start(std::slice::from_ref(&gone)).unwrap();
        assert_eq!(watch.missing_roots(), [gone]);
    }
}
