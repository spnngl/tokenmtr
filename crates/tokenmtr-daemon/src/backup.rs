//! Daily backup (D37): a compressed, integrity-checked snapshot, pruned to the newest `keep`.

use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_platform::ThreadPriority;
use tokenmtr_store::{BackupRun, StoreRead, StoreReader};

use crate::alerts::Evaluation;
use crate::changes::std_duration;
use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

const PREFIX: &str = "usage-";
const SUFFIX: &str = ".db.zst";

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
        let dest = dir.join(format!(
            "{PREFIX}{}{SUFFIX}",
            started.strftime("%Y%m%dT%H%M%SZ")
        ));
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
            prune(&dir, active.config.backup.keep);
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
        self.active()
            .config
            .backup
            .dir
            .clone()
            .unwrap_or_else(|| self.settings.paths.data_dir().join("backups"))
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

/// Backup files of `dir`, oldest first (the UTC timestamp in the name sorts chronologically).
fn backup_files(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = dir.read_dir_utf8() else {
        return Vec::new();
    };
    let mut files: Vec<Utf8PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            name.starts_with(PREFIX) && name.ends_with(SUFFIX)
        })
        .map(|entry| entry.path().to_owned())
        .collect();
    files.sort();
    files
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

/// Deletes all but the newest `keep` backups.
fn prune(dir: &Utf8Path, keep: u32) {
    let files = backup_files(dir);
    let excess = files
        .len()
        .saturating_sub(usize::try_from(keep).unwrap_or(usize::MAX));
    for old in files.into_iter().take(excess) {
        if let Err(error) = fs::remove_file(&old) {
            tracing::warn!(%error, path = %old, "cannot remove an old backup");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path()).unwrap();
        for name in [
            "usage-20260101T000000Z.db.zst",
            "usage-20260102T000000Z.db.zst",
            "usage-20260103T000000Z.db.zst",
            "notes.txt",
        ] {
            fs::write(path.join(name), b"x").unwrap();
        }
        prune(path, 2);
        let left: Vec<String> = backup_files(path)
            .iter()
            .filter_map(|file| file.file_name().map(str::to_owned))
            .collect();
        assert_eq!(
            left,
            [
                "usage-20260102T000000Z.db.zst",
                "usage-20260103T000000Z.db.zst"
            ]
        );
        assert!(path.join("notes.txt").exists());
    }
}
