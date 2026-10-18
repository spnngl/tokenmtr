//! Backup files: naming, listing and pruning. The daemon and the CLI share them.

use std::fs;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use serde::Serialize;

const PREFIX: &str = "usage-";
const SUFFIX: &str = ".db.zst";

/// A backup file on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupFile {
    /// Where it is.
    pub path: Utf8PathBuf,
    /// Its size.
    pub size_bytes: u64,
}

/// The file a backup started at `started` is written to (the UTC time sorts chronologically).
#[must_use]
pub fn backup_path(dir: &Utf8Path, started: Timestamp) -> Utf8PathBuf {
    dir.join(format!(
        "{PREFIX}{}{SUFFIX}",
        started.strftime("%Y%m%dT%H%M%SZ")
    ))
}

/// Backup files of `dir`, oldest first.
#[must_use]
pub fn backup_files(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
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

/// Backup files of `dir` with their sizes, newest first.
#[must_use]
pub fn list_backups(dir: &Utf8Path) -> Vec<BackupFile> {
    let mut files: Vec<BackupFile> = backup_files(dir)
        .into_iter()
        .map(|path| BackupFile {
            size_bytes: fs::metadata(&path).map_or(0, |meta| meta.len()),
            path,
        })
        .collect();
    files.reverse();
    files
}

/// Deletes all but the newest `keep` backups of `dir`. Returns the files that could not be
/// deleted.
#[must_use]
pub fn prune_backups(dir: &Utf8Path, keep: u32) -> Vec<(Utf8PathBuf, io::Error)> {
    let files = backup_files(dir);
    let excess = files
        .len()
        .saturating_sub(usize::try_from(keep).unwrap_or(usize::MAX));
    files
        .into_iter()
        .take(excess)
        .filter_map(|old| fs::remove_file(&old).err().map(|error| (old, error)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn prune_keeps_the_newest() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = Utf8Path::from_path(dir.path()).ok_or("temp dir is not UTF-8")?;
        for name in [
            "usage-20260101T000000Z.db.zst",
            "usage-20260102T000000Z.db.zst",
            "usage-20260103T000000Z.db.zst",
            "notes.txt",
        ] {
            fs::write(path.join(name), b"x")?;
        }
        assert!(prune_backups(path, 2).is_empty());
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
        Ok(())
    }

    #[test]
    fn list_is_newest_first_with_sizes() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = Utf8Path::from_path(dir.path()).ok_or("temp dir is not UTF-8")?;
        fs::write(path.join("usage-20260101T000000Z.db.zst"), b"x")?;
        fs::write(path.join("usage-20260102T000000Z.db.zst"), b"xyz")?;
        let listed = list_backups(path);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].size_bytes, 3);
        assert!(listed[0].path.as_str().ends_with("20260102T000000Z.db.zst"));
        Ok(())
    }

    #[test]
    fn path_carries_the_utc_start() -> TestResult {
        let at: Timestamp = "2026-05-04T03:02:01Z".parse()?;
        assert_eq!(
            backup_path(Utf8Path::new("/b"), at),
            Utf8Path::new("/b/usage-20260504T030201Z.db.zst")
        );
        Ok(())
    }
}
