//! Daily backup (D37): a compressed, integrity-checked snapshot, pruned to the newest `keep`.

use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_app::{backup_dir, backup_files, backup_path, prune_backups};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_store::{BackupRun, StoreRead, StoreReader};

use crate::alerts::Evaluation;
use crate::changes::std_duration;
use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

/// A backup that was written.
#[derive(Debug, Clone)]
pub(crate) struct BackupOutcome {
    pub path: Utf8PathBuf,
    pub size_bytes: u64,
}

impl Ctx {
    /// Writes one backup at `priority` and records it.
    pub(crate) async fn backup(
        self: &Arc<Self>,
        priority: ThreadPriority,
    ) -> Result<BackupOutcome, DaemonError> {
        self.blocking(priority, Self::backup_blocking).await?
    }

    fn backup_blocking(&self) -> Result<BackupOutcome, DaemonError> {
        let active = self.active();
        let dir = self.backup_dir();
        let started = Timestamp::now();
        let dest = backup_path(&dir, started);
        // A connection of its own: the backup never holds the reader other tasks use.
        let result = StoreReader::open(&self.store_path)
            .and_then(|reader| reader.backup(&dest))
            .map_err(DaemonError::from);
        let finished = Timestamp::now();
        let run = match &result {
            Ok(info) => BackupRun {
                started,
                finished,
                path: Some(dest.clone()),
                size_bytes: Some(info.compressed_bytes),
                error: None,
            },
            Err(error) => BackupRun {
                started,
                finished,
                path: None,
                size_bytes: None,
                error: Some(error.to_string()),
            },
        };
        if let Err(error) = lock(&self.store).record_backup_run(&run) {
            tracing::warn!(%error, "cannot record the backup run");
        }
        {
            let mut state = lock(&self.run);
            match &result {
                Ok(_) => state.backup_failed_at = None,
                // Keep the start of the streak: one alert per streak (the dedup key holds it).
                Err(_) => {
                    state.backup_failed_at.get_or_insert(finished);
                }
            }
        }
        if result.is_ok() {
            for (path, error) in prune_backups(&dir, active.config.backup.keep) {
                tracing::warn!(%error, %path, "cannot remove an old backup");
            }
        }
        self.evaluate_alerts(Evaluation::default());
        let info = result?;
        tracing::info!(path = %dest, bytes = info.compressed_bytes, "backup written");
        Ok(BackupOutcome {
            path: dest,
            size_bytes: info.compressed_bytes,
        })
    }

    /// Where backups go.
    fn backup_dir(&self) -> Utf8PathBuf {
        backup_dir(&self.settings.paths, &self.active().config)
    }

    /// How long until the next scheduled backup: the interval after the newest backup file, or a
    /// short delay when there is none (so a new install is backed up soon after it has data).
    pub(crate) fn backup_due_in(&self) -> Duration {
        let interval = std_duration(self.active().config.backup.interval);
        let newest = newest_backup_age(&self.backup_dir());
        match newest {
            Some(age) => interval.saturating_sub(age),
            None => self.settings.timing.first_backup_delay,
        }
    }
}

fn newest_backup_age(dir: &Utf8Path) -> Option<Duration> {
    let newest = backup_files(dir).pop()?;
    let modified = fs::metadata(&newest)
        .and_then(|meta| meta.modified())
        .ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}
