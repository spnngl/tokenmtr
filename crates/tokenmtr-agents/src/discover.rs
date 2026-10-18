//! Finding an agent's source files under its profile roots.

use std::io;

use camino::Utf8PathBuf;

use crate::AgentSource;
use crate::drift::ParseIssue;
use crate::source::SourceFile;

/// The files found under every profile root, and what could not be listed.
#[derive(Debug, Default)]
pub struct Discovery {
    /// Files the source [`owns`](AgentSource::owns), sorted by path, each once.
    pub files: Vec<SourceFile>,
    /// Directories that could not be read. A missing root is not one.
    pub issues: Vec<ParseIssue>,
}

/// Walks every root of every profile of `source` recursively and keeps the files it owns.
/// Symlinks are not followed, except for a root itself.
#[must_use]
pub fn discover(source: &impl AgentSource) -> Discovery {
    let mut found = Discovery::default();
    for profile in source.profiles() {
        for root in &profile.roots {
            let mut pending = vec![root.clone()];
            while let Some(dir) = pending.pop() {
                walk_dir(source, &profile.id, dir, &mut pending, &mut found);
            }
        }
    }
    found.files.sort_by(|a, b| a.path.cmp(&b.path));
    found.files.dedup_by(|a, b| a.path == b.path);
    found
}

fn walk_dir(
    source: &impl AgentSource,
    profile: &tokenmtr_core::ProfileId,
    dir: Utf8PathBuf,
    pending: &mut Vec<Utf8PathBuf>,
    found: &mut Discovery,
) {
    let entries = match dir.read_dir_utf8() {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            found.issues.push(unreadable(dir, &error));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                found.issues.push(unreadable(dir.clone(), &error));
                continue;
            }
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.into_path();
        if kind.is_dir() {
            pending.push(path);
        } else if kind.is_file() && source.owns(&path) {
            found.files.push(SourceFile::new(path, profile.clone()));
        }
    }
}

fn unreadable(path: Utf8PathBuf, error: &io::Error) -> ParseIssue {
    ParseIssue::Unreadable {
        path,
        reason: error.to_string(),
    }
}
