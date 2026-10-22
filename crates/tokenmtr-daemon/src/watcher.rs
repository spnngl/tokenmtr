//! The file watcher task: agent roots feed debounced scans, the config file feeds reloads.
//!
//! One `notify` watcher whose callback pushes into a bounded channel; when the channel is full the
//! event is dropped and the next batch becomes a full rescan. The task sleeps until the
//! debouncer's next deadline, and while some agent root is missing until the next root poll, so
//! with no file activity and every root present it never wakes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokenmtr_agents::ingest::watch::{RawEvent, WatchBatch, WatchCore};
use tokenmtr_agents::ingest::{ScanPriority, ScanRequest};
use tokenmtr_control::{Shutdown, wait_shutdown};
use tokio::sync::mpsc;

use crate::ctx::{Ctx, Job, WatchPlan};
use crate::reload::owns;

type Raw = notify::Result<notify::Event>;

/// Raw events buffered between the `notify` thread and the task; beyond that a rescan replaces
/// them.
const CHANNEL_CAPACITY: usize = 1024;

/// The `notify` side of the channel: a full channel drops the event and raises `overflowed`.
#[derive(Clone)]
struct Inbox {
    tx: mpsc::Sender<Raw>,
    overflowed: Arc<AtomicBool>,
}

impl Inbox {
    fn push(&self, event: Raw) {
        match self.tx.try_send(event) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {} // closed: shutting down
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflowed.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Whether events were dropped since the last call.
    fn take_overflow(&self) -> bool {
        self.overflowed.swap(false, Ordering::Relaxed)
    }
}

/// What is being watched right now.
struct Live {
    /// Dropping it stops the events.
    watcher: Option<RecommendedWatcher>,
    /// Maps the real path the OS reports back to the configured root (they differ behind a
    /// symlink, e.g. `/var` → `/private/var` on macOS).
    remap: Vec<(Utf8PathBuf, Utf8PathBuf)>,
    /// Roots that did not exist when last tried; polled every [`Timing::root_poll`].
    ///
    /// [`Timing::root_poll`]: crate::Timing::root_poll
    missing: Vec<Utf8PathBuf>,
    /// The config file as configured and as the OS reports it (behind a symlinked parent).
    config_paths: Vec<Utf8PathBuf>,
}

impl Live {
    fn start(ctx: &Ctx, plan: &WatchPlan, inbox: &Inbox) -> Self {
        let sender = inbox.clone();
        let watcher = notify::recommended_watcher(move |event| sender.push(event));
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::error!(%error, "cannot start the file watcher; only the sweep finds changes");
                return Self {
                    watcher: None,
                    remap: Vec::new(),
                    missing: Vec::new(),
                    config_paths: Vec::new(),
                };
            }
        };
        let config_paths = watch_config(&mut watcher, &ctx.settings.config_path);
        let mut live = Self {
            watcher: Some(watcher),
            remap: Vec::new(),
            missing: plan.roots.clone(),
            config_paths,
        };
        live.watch_appeared();
        live
    }

    /// Starts watching the missing roots that exist now; returns whether any did.
    fn watch_appeared(&mut self) -> bool {
        let Some(watcher) = &mut self.watcher else {
            return false;
        };
        let before = self.missing.len();
        self.missing.retain(|root| {
            if !root.is_dir() {
                tracing::debug!(%root, "agent root does not exist yet; polling for it");
                return true;
            }
            if let Err(error) = watcher.watch(root.as_std_path(), RecursiveMode::Recursive) {
                // Not retried: the sweep still covers it.
                tracing::warn!(%error, %root, "cannot watch an agent root");
                return false;
            }
            if let Ok(real) = root.canonicalize_utf8()
                && real != *root
            {
                self.remap.push((real, root.clone()));
            }
            false
        });
        self.missing.len() < before
    }

    /// When to poll the missing roots next: never while every root is watched, else at
    /// `current` if still ahead, else one `period` from now.
    fn next_poll(
        &self,
        current: Option<tokio::time::Instant>,
        period: Duration,
    ) -> Option<tokio::time::Instant> {
        if self.missing.is_empty() {
            return None;
        }
        let now = tokio::time::Instant::now();
        Some(current.filter(|at| *at > now).unwrap_or(now + period))
    }

    fn touches_config(&self, event: &notify::Event) -> bool {
        event.paths.iter().any(|path| {
            self.config_paths
                .iter()
                .any(|config| config.as_std_path() == path)
        })
    }

    fn remapped(&self, path: Utf8PathBuf) -> Utf8PathBuf {
        for (real, configured) in &self.remap {
            if let Ok(rest) = path.strip_prefix(real) {
                return configured.join(rest);
            }
        }
        path
    }
}

/// Watches the directory of the config file, creating it so a config written later is seen;
/// returns the paths events may report for the file.
fn watch_config(watcher: &mut RecommendedWatcher, config: &Utf8Path) -> Vec<Utf8PathBuf> {
    let (Some(dir), Some(name)) = (config.parent(), config.file_name()) else {
        return Vec::new();
    };
    if let Err(error) = std::fs::create_dir_all(dir) {
        tracing::warn!(%error, %dir, "cannot create the config directory");
    }
    if let Err(error) = watcher.watch(dir.as_std_path(), RecursiveMode::NonRecursive) {
        tracing::warn!(%error, "cannot watch the config file");
    }
    let mut paths = vec![config.to_owned()];
    if let Ok(real) = dir.canonicalize_utf8()
        && real != *dir
    {
        paths.push(real.join(name));
    }
    paths
}

/// Whether an agent of the current watch plan owns `path`. Read live: a reload that keeps the
/// roots updates ownership without notifying the watcher.
fn owned(ctx: &Ctx, path: &Utf8Path) -> bool {
    ctx.watch_plan
        .borrow()
        .owner
        .as_ref()
        .is_some_and(|settings| owns(settings, path))
}

/// Runs until `shutdown`.
pub(crate) async fn run(ctx: Arc<Ctx>, mut shutdown: Shutdown) {
    let mut plan = ctx.watch_plan.subscribe();
    let (tx, mut rx) = mpsc::channel::<Raw>(CHANNEL_CAPACITY);
    let inbox = Inbox {
        tx,
        overflowed: Arc::default(),
    };
    let mut core = WatchCore::new(ctx.settings.timing.watch);
    let initial = plan.borrow_and_update().clone();
    let mut live = Live::start(&ctx, &initial, &inbox);
    let poll_period = ctx.settings.timing.root_poll;
    let mut next_poll = live.next_poll(None, poll_period);
    loop {
        while let Some(batch) = core.poll(Instant::now()) {
            let request = match batch {
                WatchBatch::Rescan => ScanRequest::incremental(ScanPriority::Background),
                WatchBatch::Paths(paths) => {
                    ScanRequest::incremental(ScanPriority::Background).for_paths(paths)
                }
            };
            tokio::select! {
                () = wait_shutdown(&mut shutdown) => return,
                result = ctx.scan(request) => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "scan after a file change failed");
                    }
                }
            }
        }
        let hint = core.sleep_hint(Instant::now());
        let wake = async {
            match hint {
                Some(delay) => tokio::time::sleep(delay).await,
                None => std::future::pending().await,
            }
        };
        let poll = async {
            match next_poll {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = wait_shutdown(&mut shutdown) => return,
            changed = plan.changed() => {
                if changed.is_err() {
                    return;
                }
                let next = plan.borrow_and_update().clone();
                live = Live::start(&ctx, &next, &inbox);
                // Files may have appeared in roots the old watch did not see.
                core.feed(RawEvent::Rescan, Instant::now(), &|_| false);
            }
            event = rx.recv() => match event {
                Some(event) => {
                    feed(&ctx, &live, &mut core, event);
                    // A full channel means at least one event was dropped after the one read.
                    if inbox.take_overflow() {
                        tracing::debug!("file watcher channel overflowed, scheduling a rescan");
                        core.feed(RawEvent::Rescan, Instant::now(), &|_| false);
                    }
                }
                None => return,
            },
            () = poll => {
                if live.watch_appeared() {
                    // Files written before the watch started are found by a rescan.
                    core.feed(RawEvent::Rescan, Instant::now(), &|_| false);
                }
            }
            () = wake => {}
        }
        next_poll = live.next_poll(next_poll, poll_period);
    }
}

fn feed(ctx: &Ctx, live: &Live, core: &mut WatchCore, event: Raw) {
    let raw = match event {
        Ok(event) => {
            if live.touches_config(&event) {
                ctx.scheduler.schedule_in(
                    Job::Reload,
                    ctx.settings.timing.reload_debounce,
                    std::time::Duration::ZERO,
                );
            }
            RawEvent::from_notify(&event)
        }
        Err(error) => {
            tracing::warn!(%error, "file watcher error, scheduling a rescan");
            Some(RawEvent::Rescan)
        }
    };
    let raw = match raw {
        Some(RawEvent::Changed(paths)) => {
            let paths: Vec<Utf8PathBuf> =
                paths.into_iter().map(|path| live.remapped(path)).collect();
            wake_imports(ctx, Some(&paths));
            RawEvent::Changed(paths)
        }
        Some(RawEvent::Rescan) => {
            wake_imports(ctx, None);
            RawEvent::Rescan
        }
        None => return,
    };
    core.feed(raw, Instant::now(), &|path| owned(ctx, path));
}

/// Schedules the import of every source a changed path belongs to; `None` (events were lost)
/// wakes them all. The debounce lets a sync client finish writing the file.
fn wake_imports(ctx: &Ctx, changed: Option<&[Utf8PathBuf]>) {
    for watch in &ctx.watch_plan.borrow().imports {
        let hit = changed.is_none_or(|paths| {
            paths.iter().any(|path| {
                watch.include.is_match(path.as_str()) && !watch.exclude.is_match(path.as_str())
            })
        });
        if hit {
            ctx.scheduler.schedule_in(
                Job::Import(watch.source.clone()),
                ctx.settings.timing.import_debounce,
                std::time::Duration::ZERO,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_inbox_drops_the_event_and_flags_one_rescan() {
        let (tx, mut rx) = mpsc::channel::<Raw>(1);
        let inbox = Inbox {
            tx,
            overflowed: Arc::default(),
        };
        inbox.push(Ok(notify::Event::default()));
        assert!(!inbox.take_overflow());
        inbox.push(Ok(notify::Event::default()));
        assert!(inbox.take_overflow(), "the second event did not fit");
        assert!(!inbox.take_overflow(), "the flag is taken once");
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "the dropped event is gone");
        drop(rx);
        inbox.push(Ok(notify::Event::default()));
        assert!(!inbox.take_overflow(), "a closed channel is no overflow");
    }
}
