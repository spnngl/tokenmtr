//! Where an agent's data lives: profiles, source files and byte offsets.

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use tokenmtr_core::{ProfileId, SourceFileId};

/// A byte position in a source file. A checkpoint stores one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize)]
#[serde(transparent)]
pub struct ByteOffset(u64);

impl ByteOffset {
    /// The start of a file.
    pub const ZERO: Self = Self(0);

    /// Wraps a byte position.
    #[must_use]
    pub const fn new(offset: u64) -> Self {
        Self(offset)
    }

    /// The byte position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A named group of one agent's roots with its own credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Profile id, `default` unless configured.
    pub id: ProfileId,
    /// Directories walked for source files. A missing root is not an error.
    pub roots: Vec<Utf8PathBuf>,
}

impl Profile {
    /// A profile with the given roots.
    #[must_use]
    pub const fn new(id: ProfileId, roots: Vec<Utf8PathBuf>) -> Self {
        Self { id, roots }
    }

    /// Whether `path` lies under one of the roots.
    #[must_use]
    pub fn contains(&self, path: &Utf8Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }
}

/// One on-disk file an agent wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Absolute path of the file.
    pub path: Utf8PathBuf,
    /// Profile whose root contains the file.
    pub profile: ProfileId,
    /// Store row id once the file is registered; copied onto events.
    pub id: Option<SourceFileId>,
}

impl SourceFile {
    /// A file not yet registered in the store.
    #[must_use]
    pub const fn new(path: Utf8PathBuf, profile: ProfileId) -> Self {
        Self {
            path,
            profile,
            id: None,
        }
    }
}
