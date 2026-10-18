//! Durable file writes (D49).
//!
//! On Apple targets `File::sync_all` issues `fcntl(F_FULLFSYNC)`, which is the only call that
//! flushes the drive cache, so every durability point here uses it.

use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::NamedTempFile;

/// Replaces `path` with `bytes` so readers see either the old or the new content, never a mix,
/// even across a crash.
///
/// Writes a temp file in the same directory, syncs it, renames it over `path`, then syncs the
/// parent directory so the rename itself is durable. On error the temp file is removed and
/// `path` is untouched.
///
/// When `path` is a symbolic link, the file it points to is replaced and the link is kept. The
/// new file keeps the permission bits of the file it replaces (`0600` for a new file). Owner,
/// ACLs and extended attributes are not carried over.
///
/// # Errors
/// Any I/O error from resolving, creating, writing, syncing or renaming; the parent directory
/// must exist.
pub fn replace_atomically(path: &Utf8Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_link(path)?;
    let parent = match target.parent() {
        Some(p) if !p.as_str().is_empty() => p,
        _ => Utf8Path::new("."),
    };
    let mut temp = NamedTempFile::new_in(parent)?;
    match fs::metadata(&target) {
        Ok(old) => temp
            .as_file()
            .set_permissions(Permissions::from_mode(old.permissions().mode() & 0o7777))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(&target).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()
}

/// The file `path` names: the final target of a symbolic link (one hop when the link dangles),
/// else `path` itself.
fn resolve_link(path: &Utf8Path) -> io::Result<Utf8PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {}
        Ok(_) => return Ok(path.to_owned()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(path.to_owned()),
        Err(e) => return Err(e),
    }
    let resolved = match fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
            parent.as_std_path().join(fs::read_link(path)?)
        }
        Err(e) => return Err(e),
    };
    Utf8PathBuf::from_path_buf(resolved)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "link target is not UTF-8"))
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
    fn keeps_the_mode_of_the_replaced_file() {
        let (_guard, dir) = tempdir();
        let target = dir.join("settings.json");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, Permissions::from_mode(0o644)).unwrap();
        replace_atomically(&target, b"new").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o644);
    }

    #[test]
    fn a_new_file_is_private() {
        let (_guard, dir) = tempdir();
        let target = dir.join("new.json");
        replace_atomically(&target, b"new").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn replaces_the_target_of_a_symlink_and_keeps_the_link() {
        let (_guard, dir) = tempdir();
        std::fs::create_dir(dir.join("real")).unwrap();
        let real = dir.join("real/settings.json");
        std::fs::write(&real, b"old").unwrap();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink("real/settings.json", &link).unwrap();

        replace_atomically(&link, b"new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
        assert_eq!(entries(&dir.join("real")), ["settings.json"]);
    }

    #[test]
    fn a_dangling_symlink_gets_its_target_created() {
        let (_guard, dir) = tempdir();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink("target.json", &link).unwrap();
        replace_atomically(&link, b"new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(dir.join("target.json")).unwrap(), b"new");
    }

    #[test]
    fn missing_parent_is_an_error() {
        let (_guard, dir) = tempdir();
        assert!(replace_atomically(&dir.join("nope/file"), b"x").is_err());
    }
}
