//! Backup and restore of the database file (D37, D49).
//!
//! A backup is a consistent snapshot taken with `SQLite`'s online backup API, so it works while
//! the daemon writes. The snapshot is checked with `PRAGMA integrity_check`, compressed with zstd
//! and moved into place with the fsync dance of D49: write a temporary file next to the target,
//! `sync_all`, rename, sync the directory.

use std::fs::{self, File};
use std::io;
use std::time::Duration;

use camino::Utf8Path;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags, ffi};
use tempfile::NamedTempFile;

use crate::db::{check_integrity, reject_newer_schema};
use crate::error::StoreError;

/// zstd level of backup files: databases compress well already at the default.
const ZSTD_LEVEL: i32 = 3;

/// How long the online backup waits before retrying when the source is locked.
const RETRY_PAUSE: Duration = Duration::from_millis(50);

/// How many times the online backup retries a locked source before giving up (about 5 s).
const MAX_ATTEMPTS: u32 = 100;

/// Copies the whole of `source` into `copy` in one step, retrying while a lock blocks it.
fn copy_database(source: &Connection, copy: &mut Connection) -> Result<(), StoreError> {
    let backup = Backup::new(source, copy)?;
    for _ in 0..MAX_ATTEMPTS {
        match backup.step(-1)? {
            StepResult::Done => return Ok(()),
            _ => std::thread::sleep(RETRY_PAUSE),
        }
    }
    Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        ffi::Error::new(ffi::SQLITE_BUSY),
        None,
    )))
}

/// What a backup wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupInfo {
    /// Size of the database snapshot before compression.
    pub raw_bytes: u64,
    /// Size of the file written.
    pub compressed_bytes: u64,
}

fn parent_of(path: &Utf8Path) -> &Utf8Path {
    match path.parent() {
        Some(parent) if !parent.as_str().is_empty() => parent,
        _ => Utf8Path::new("."),
    }
}

fn sync_directory(dir: &Utf8Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Writes a zstd-compressed, integrity-checked snapshot of `source` to `dest`, atomically.
pub(crate) fn backup(source: &Connection, dest: &Utf8Path) -> Result<BackupInfo, StoreError> {
    let parent = parent_of(dest);
    fs::create_dir_all(parent)?;
    let scratch = tempfile::tempdir_in(parent)?;
    let snapshot = scratch.path().join("snapshot.db");
    {
        let mut copy = Connection::open(&snapshot)?;
        copy_database(source, &mut copy)?;
        // The copy inherits WAL mode; make it a single self-contained file.
        copy.pragma_update_and_check(None, "journal_mode", "DELETE", |row| {
            row.get::<_, String>(0)
        })?;
        check_integrity(&copy)?;
        copy.close().map_err(|(_, error)| error)?;
    }
    let mut raw = File::open(&snapshot)?;
    let raw_bytes = raw.metadata()?.len();
    let mut staged = NamedTempFile::new_in(parent)?;
    zstd::stream::copy_encode(&mut raw, staged.as_file_mut(), ZSTD_LEVEL)?;
    staged.as_file().sync_all()?;
    let compressed_bytes = staged.as_file().metadata()?.len();
    staged.persist(dest).map_err(|e| e.error)?;
    sync_directory(parent)?;
    Ok(BackupInfo {
        raw_bytes,
        compressed_bytes,
    })
}

/// Replaces the database at `dest` with the backup `src`, atomically.
///
/// The backup is decompressed next to `dest`, integrity-checked and refused when its schema is
/// newer than this build's. Stale `-wal` and `-shm` files of `dest` are removed, so the caller
/// must make sure no process has `dest` open (the CLI checks that the daemon is stopped).
pub fn restore(src: &Utf8Path, dest: &Utf8Path) -> Result<(), StoreError> {
    let parent = parent_of(dest);
    fs::create_dir_all(parent)?;
    let mut staged = NamedTempFile::new_in(parent)?;
    zstd::stream::copy_decode(File::open(src)?, staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    {
        let check = Connection::open_with_flags(staged.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        reject_newer_schema(&check)?;
        check_integrity(&check)?;
    }
    // The stale WAL goes first: a crash before the rename leaves the old database at its last
    // checkpoint, still consistent, whereas the new file next to the old WAL would replay foreign
    // frames on open.
    for suffix in ["-wal", "-shm"] {
        match fs::remove_file(format!("{dest}{suffix}")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    staged.persist(dest).map_err(|e| e.error)?;
    sync_directory(parent)?;
    Ok(())
}
