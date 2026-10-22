//! Finds the files a source's `include` and `exclude` globs name.

use std::collections::BTreeSet;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_config::GlobList;

use crate::error::ImportError;

/// Deepest folder a `**` is followed into.
const MAX_DEPTH: usize = 16;

/// Files matching `include` and not `exclude`, sorted; and the folders that could not be read.
/// A folder that does not exist is not a failure: a sync client may not have created it yet.
#[must_use]
pub fn matching_files(
    include: &GlobList,
    exclude: &GlobList,
) -> (Vec<Utf8PathBuf>, Vec<(Utf8PathBuf, ImportError)>) {
    let mut files = BTreeSet::new();
    let mut failures = Vec::new();
    for pattern in include.patterns() {
        let (root, depth) = walk_plan(pattern);
        walk(
            &root,
            depth,
            &mut |path| {
                if include.is_match(path.as_str()) && !exclude.is_match(path.as_str()) {
                    files.insert(path);
                }
            },
            &mut failures,
        );
    }
    (files.into_iter().collect(), failures)
}

fn has_meta(segment: &str) -> bool {
    segment.contains(['*', '?', '[', ']', '{', '}'])
}

/// The folder to start from (the pattern's literal prefix) and how many levels to go down.
fn walk_plan(pattern: &str) -> (Utf8PathBuf, usize) {
    let segments: Vec<&str> = pattern.split('/').collect();
    let literal = segments.iter().take_while(|s| !has_meta(s)).count();
    let rest = &segments[literal..];
    if rest.is_empty() {
        // No glob at all: the pattern names one file, found by listing its folder.
        let parent = Utf8Path::new(pattern)
            .parent()
            .unwrap_or(Utf8Path::new("/"));
        return (parent.to_owned(), 1);
    }
    let depth = if rest.iter().any(|s| s.contains("**")) {
        MAX_DEPTH
    } else {
        rest.len()
    };
    let root = segments[..literal].join("/");
    (
        if root.is_empty() {
            Utf8PathBuf::from("/")
        } else {
            Utf8PathBuf::from(root)
        },
        depth,
    )
}

fn walk(
    dir: &Utf8Path,
    depth: usize,
    found: &mut dyn FnMut(Utf8PathBuf),
    failures: &mut Vec<(Utf8PathBuf, ImportError)>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            failures.push((dir.to_owned(), ImportError::io(error)));
            return;
        }
    };
    for entry in entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 1 {
                walk(&path, depth - 1, found, failures);
            }
        } else if fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            found(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_starts_at_the_literal_prefix() {
        assert_eq!(walk_plan("/a/b/*.db"), (Utf8PathBuf::from("/a/b"), 1));
        assert_eq!(walk_plan("/a/*/x/*.db"), (Utf8PathBuf::from("/a"), 3));
        assert_eq!(
            walk_plan("/a/**/*.db"),
            (Utf8PathBuf::from("/a"), MAX_DEPTH)
        );
        assert_eq!(walk_plan("/a/b/one.db"), (Utf8PathBuf::from("/a/b"), 1));
        assert_eq!(walk_plan("/*.db"), (Utf8PathBuf::from("/"), 1));
    }
}
