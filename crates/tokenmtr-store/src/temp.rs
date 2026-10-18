//! A throw-away store for tests of this crate's consumers.
//!
//! There is deliberately no `StoreReader::open_in_memory`. The store requires WAL, and WAL
//! needs a real file: an in-memory database cannot use it, and a shared-cache in-memory database
//! has table-level locks instead of snapshot reads, so it would not behave like the writer/reader
//! pair of production (a reader would block, or be blocked by, the open write transaction).
//! [`TempStore`] gives the same two-connection setup on a temporary directory instead.

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;

use crate::db::{Store, StoreReader};
use crate::error::StoreError;

/// A [`Store`] in a temporary directory that is deleted on drop, with readers on the same file.
#[derive(Debug)]
pub struct TempStore {
    store: Store,
    path: Utf8PathBuf,
    // Declared last: dropped after the connection is closed.
    _dir: TempDir,
}

impl TempStore {
    /// Creates and migrates a fresh database in a new temporary directory.
    ///
    /// # Errors
    /// File and `SQLite` failures, or a temporary directory path that is not UTF-8.
    pub fn new() -> Result<Self, StoreError> {
        let dir = tempfile::tempdir()?;
        let path = Utf8PathBuf::from_path_buf(dir.path().join("store.db")).map_err(|path| {
            StoreError::Io(std::io::Error::other(format!(
                "temporary path is not UTF-8: {}",
                path.display()
            )))
        })?;
        let store = Store::open(&path)?;
        Ok(Self {
            store,
            path,
            _dir: dir,
        })
    }

    /// The writer.
    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    /// The writer, mutably.
    pub const fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// A new read-only connection on the same file.
    ///
    /// # Errors
    /// `SQLite` failures.
    pub fn reader(&self) -> Result<StoreReader, StoreError> {
        StoreReader::open(&self.path)
    }
}
