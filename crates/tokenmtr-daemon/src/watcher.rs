//! The file watcher task (D16): agent roots feed debounced scans, the config file feeds reloads.
//!
//! One `notify` watcher whose callback pushes into an unbounded channel. The task sleeps until
//! the debouncer's next deadline, so with no file activity it never wakes (D47).

use std::sync::Arc;
use std::time::Instant;

use camino::{Utf8Path, Utf8PathBuf};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokenmtr_agents::ingest::watch::{RawEvent, WatchBatch, WatchCore};
use tokenmtr_agents::ingest::{IngestSettings, ScanPriority, ScanRequest};
use tokenmtr_control::{Shutdown, wait_shutdown};
use tokio::sync::mpsc;

use crate::ctx::{Ctx, Job, WatchPlan};
use crate::reload::owns;

type Raw = notify::Result<notify::Event>;

/// What is being watched right now.
struct Live {
    /// Dropping it stops the events.
    _watcher: Option<RecommendedWatcher>,
    /// Maps the real path the OS reports back to the configured root (they differ behind a
    /// symlink, e.g. `/var` → `/private/var` on macOS).
    remap: Vec<(Utf8PathBuf, Utf8PathBuf)>,
    owner: Option<IngestSettings>,
    config_name: Option<String>,
}

impl Live {
    fn start(ctx: &Ctx, plan: &WatchPlan, tx: &mpsc::UnboundedSender<Raw>) -> Self {
        let sender = tx.clone();
        let watcher = notify::recommended_watcher(move |event| {
            // The receiver is gone only during shutdown.
            let _ = sender.send(event);
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::error!(%error, "cannot start the file watcher; only the sweep finds changes");
                return Self {
                    _watcher: None,
                    remap: Vec::new(),
                    owner: plan.owner.clone(),
                    config_name: None,
                };
            }
        };
        let mut remap = Vec::new();
        for root in &plan.roots {
            if !root.is_dir() {
                tracing::debug!(%root, "agent root does not exist yet; the sweep will find it");
                continue;
            }
            if let Err(error) = watcher.watch(root.as_std_path(), RecursiveMode::Recursive) {
                tracing::warn!(%error, %root, "cannot watch an agent root");
                continue;
            }
            if let Ok(real) = root.canonicalize_utf8()
                && real != *root
            {
                remap.push((real, root.clone()));
            }
        }
        let config = &ctx.settings.config_path;
        let config_name = config.file_name().map(str::to_owned);
        if let Some(dir) = config.parent().filter(|dir| dir.is_dir())
            && let Err(error) = watcher.watch(dir.as_std_path(), RecursiveMode::NonRecursive)
        {
            tracing::warn!(%error, "cannot watch the config file");
        }
        Self {
            _watcher: Some(watcher),
            remap,
            owner: plan.owner.clone(),
            config_name,
        }
    }

    fn touches_config(&self, event: &notify::Event) -> bool {
        let Some(name) = &self.config_name else {
            return false;
        };
        event
            .paths
            .iter()
            .any(|path| path.file_name().is_some_and(|file| file == name.as_str()))
    }

    fn remapped(&self, path: Utf8PathBuf) -> Utf8PathBuf {
        for (real, configured) in &self.remap {
            if let Ok(rest) = path.strip_prefix(real) {
                return configured.join(rest);
            }
        }
        path
    }

    fn owns(&self, path: &Utf8Path) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|settings| owns(settings, path))
    }
}

/// Runs until `shutdown`.
pub(crate) async fn run(ctx: Arc<Ctx>, mut shutdown: Shutdown) {
    let mut plan = ctx.watch_plan.subscribe();
    let (tx, mut rx) = mpsc::unbounded_channel::<Raw>();
    let mut core = WatchCore::new(ctx.settings.timing.watch);
    let initial = plan.borrow_and_update().clone();
    let mut live = Live::start(&ctx, &initial, &tx);
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
        tokio::select! {
            () = wait_shutdown(&mut shutdown) => return,
            changed = plan.changed() => {
                if changed.is_err() {
                    return;
                }
                let next = plan.borrow_and_update().clone();
                live = Live::start(&ctx, &next, &tx);
                // Files may have appeared in roots the old watch did not see.
                core.feed(RawEvent::Rescan, Instant::now(), &|_| false);
            }
            event = rx.recv() => match event {
                Some(event) => feed(&ctx, &live, &mut core, event),
                None => return,
            },
            () = wake => {}
        }
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
            RawEvent::Changed(paths.into_iter().map(|path| live.remapped(path)).collect())
        }
        Some(other) => other,
        None => return,
    };
    core.feed(raw, Instant::now(), &|path| live.owns(path));
}
