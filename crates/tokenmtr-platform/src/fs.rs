//! Durable file writes (D49).
//!
//! On Apple targets `File::sync_all` issues `fcntl(F_FULLFSYNC)`, which is the only call that
//! flushes the drive cache, so every durability point here uses it.

use std::fs::File;
use std::io::{self, Write};

use camino::Utf8Path;
use tempfile::NamedTempFile;

/// Replaces `path` with `bytes` so readers see either the old or the new content, never a mix,
/// even across a crash.
///
/// Writes a temp file in the same directory, syncs it, renames it over `path`, then syncs the
/// parent directory so the rename itself is durable. On error the temp file is removed and
/// `path` is untouched. The resulting file has mode `0600`.
///
/// # Errors
/// Any I/O error from creating, writing, syncing or renaming; the parent directory must exist.
pub fn replace_atomically(path: &Utf8Path, bytes: &[u8]) -> io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_str().is_empty() => p,
        _ => Utf8Path::new("."),
    };
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn tempdir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        (dir, path)
    }

    fn entries(dir: &Utf8Path) -> Vec<String> {
        let mut names: Vec<_> = dir
            .read_dir_utf8()
            .unwrap()
            .map(|e| e.unwrap().file_name().to_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replaces_content_and_leaves_no_temp_file() {
        let (_guard, dir) = tempdir();
        let target = dir.join("state.json");
        replace_atomically(&target, b"old").unwrap();
        replace_atomically(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(entries(&dir), ["state.json"]);
    }

    #[test]
    fn failure_keeps_old_state_and_removes_temp_file() {
        let (_guard, dir) = tempdir();
        // A non-empty directory cannot be replaced by a file: the rename fails after the temp
        // file was fully written and synced.
        let target = dir.join("occupied");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"old").unwrap();

        assert!(replace_atomically(&target, b"new").is_err());
        assert_eq!(std::fs::read(target.join("keep")).unwrap(), b"old");
        assert_eq!(entries(&dir), ["occupied"]);
    }

    #[test]
    fn missing_parent_is_an_error() {
        let (_guard, dir) = tempdir();
        assert!(replace_atomically(&dir.join("nope/file"), b"x").is_err());
    }
}
