//! Checkpoint comparison (D15): decides, from a stored checkpoint and the file as it is now,
//! whether to skip a file, resume it at the stored offset, or parse it from zero.

use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _, SeekFrom};

use camino::Utf8Path;
use jiff::Timestamp;
use tokenmtr_store::Checkpoint;

use crate::jsonl::{HASH_WINDOW, line_hash};

/// A file's identity and size at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    /// Device id (0 where the platform has none).
    pub dev: u64,
    /// Inode (0 where the platform has none).
    pub inode: u64,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, truncated to microseconds like the store keeps it.
    pub mtime: Timestamp,
}

impl FileStat {
    /// Stats `path`, following a final symlink: the identity is that of the file read.
    pub fn of(path: &Utf8Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        let modified = meta.modified()?;
        let mtime = Timestamp::try_from(modified)
            .and_then(|ts| Timestamp::from_microsecond(ts.as_microsecond()))
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let (dev, inode) = identity(&meta);
        Ok(Self {
            dev,
            inode,
            size: meta.len(),
            mtime,
        })
    }
}

/// The `(device, inode)` of the file at `path`, following symlinks; `None` when it cannot be
/// stated or the platform has no inodes.
pub(crate) fn file_identity(path: &Utf8Path) -> Option<(u64, u64)> {
    let identity = identity(&fs::metadata(path).ok()?);
    (identity != (0, 0)).then_some(identity)
}

#[cfg(unix)]
fn identity(meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(_meta: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Why a file is parsed, and from where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseReason {
    /// No checkpoint: the first scan of the file.
    New,
    /// The file grew; parsing resumes at the stored offset.
    Appended,
    /// Another inode or device sits at the path (the file was replaced).
    Replaced,
    /// The file is smaller than the consumed offset.
    Truncated,
    /// The bytes before the offset changed (the last consumed line no longer matches).
    Rewritten,
    /// The caller asked for a full rescan.
    Forced,
}

/// What a scan does with one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Nothing changed.
    Skip,
    /// Parse from `from`.
    Parse {
        /// First byte to read.
        from: u64,
        /// Why.
        reason: ParseReason,
    },
}

/// Compares `checkpoint` with `stat` (D15). `hash_before` computes the last-line hash before an
/// offset in the file as it is now; it runs only when the file changed in place.
pub fn decide(
    checkpoint: Option<&Checkpoint>,
    stat: &FileStat,
    hash_before: impl FnOnce(u64) -> io::Result<Option<[u8; 32]>>,
) -> io::Result<Plan> {
    let Some(cp) = checkpoint else {
        return Ok(Plan::Parse {
            from: 0,
            reason: ParseReason::New,
        });
    };
    let restart = |reason| Plan::Parse { from: 0, reason };
    if cp.dev != stat.dev || cp.inode != stat.inode {
        return Ok(restart(ParseReason::Replaced));
    }
    if stat.size < cp.offset {
        return Ok(restart(ParseReason::Truncated));
    }
    if stat.size == cp.size && stat.mtime == cp.mtime {
        return Ok(Plan::Skip);
    }
    if cp.offset == 0 {
        return Ok(Plan::Parse {
            from: 0,
            reason: ParseReason::Appended,
        });
    }
    // A missing stored hash cannot vouch for the prefix.
    match cp.last_line_hash {
        Some(stored) if hash_before(cp.offset)? == Some(stored) => Ok(Plan::Parse {
            from: cp.offset,
            reason: ParseReason::Appended,
        }),
        _ => Ok(restart(ParseReason::Rewritten)),
    }
}

/// [`line_hash`] of the bytes before `offset` in the file as it is now: the last line ending
/// there, with its newline, reading at most [`HASH_WINDOW`] bytes. `None` at offset 0 or when the
/// file is shorter than `offset`.
pub fn last_line_hash(path: &Utf8Path, offset: u64) -> io::Result<Option<[u8; 32]>> {
    let window = u64::try_from(HASH_WINDOW).unwrap_or(u64::MAX);
    let start = offset.saturating_sub(window);
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let len = usize::try_from(offset - start).unwrap_or(0);
    let mut window = vec![0_u8; len];
    if file.read_exact(&mut window).is_err() {
        return Ok(None);
    }
    Ok(line_hash(&window))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use sha2::{Digest as _, Sha256};
    use tokenmtr_core::{AgentId, ProfileId};

    use super::*;
    use crate::testkit::utf8_dir;

    fn checkpoint(stat: &FileStat, offset: u64, hash: Option<[u8; 32]>) -> Checkpoint {
        Checkpoint {
            path: "/x".into(),
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            dev: stat.dev,
            inode: stat.inode,
            size: stat.size,
            mtime: stat.mtime,
            offset,
            last_line_hash: hash,
        }
    }

    fn stat(size: u64, micros: i64) -> FileStat {
        FileStat {
            dev: 1,
            inode: 2,
            size,
            mtime: Timestamp::from_microsecond(micros).unwrap(),
        }
    }

    fn never(_: u64) -> io::Result<Option<[u8; 32]>> {
        panic!("hash must not be computed");
    }

    #[test]
    fn a_new_file_parses_from_zero() {
        assert_eq!(
            decide(None, &stat(10, 1), never).unwrap(),
            Plan::Parse {
                from: 0,
                reason: ParseReason::New
            }
        );
    }

    #[test]
    fn an_unchanged_file_is_skipped_without_hashing() {
        let s = stat(10, 1);
        assert_eq!(
            decide(Some(&checkpoint(&s, 10, None)), &s, never).unwrap(),
            Plan::Skip
        );
    }

    #[test]
    fn another_inode_restarts_the_file() {
        let old = stat(10, 1);
        let new = FileStat { inode: 3, ..old };
        assert_eq!(
            decide(Some(&checkpoint(&old, 10, None)), &new, never).unwrap(),
            Plan::Parse {
                from: 0,
                reason: ParseReason::Replaced
            }
        );
    }

    #[test]
    fn a_file_smaller_than_the_offset_restarts() {
        let old = stat(100, 1);
        assert_eq!(
            decide(Some(&checkpoint(&old, 100, None)), &stat(40, 2), never).unwrap(),
            Plan::Parse {
                from: 0,
                reason: ParseReason::Truncated
            }
        );
    }

    #[test]
    fn growth_resumes_only_when_the_last_line_still_matches() {
        let old = stat(100, 1);
        let hash = [7_u8; 32];
        let cp = checkpoint(&old, 100, Some(hash));
        let grown = stat(150, 2);
        assert_eq!(
            decide(Some(&cp), &grown, |_| Ok(Some(hash))).unwrap(),
            Plan::Parse {
                from: 100,
                reason: ParseReason::Appended
            }
        );
        assert_eq!(
            decide(Some(&cp), &grown, |_| Ok(Some([8; 32]))).unwrap(),
            Plan::Parse {
                from: 0,
                reason: ParseReason::Rewritten
            }
        );
        let no_hash = checkpoint(&old, 100, None);
        assert_eq!(
            decide(Some(&no_hash), &grown, |_| Ok(Some(hash))).unwrap(),
            Plan::Parse {
                from: 0,
                reason: ParseReason::Rewritten
            },
            "a missing stored hash cannot vouch for the prefix"
        );
    }

    #[test]
    fn a_placeholder_checkpoint_never_skips() {
        let placeholder = Checkpoint {
            size: 0,
            mtime: Timestamp::UNIX_EPOCH,
            ..checkpoint(&stat(0, 0), 0, None)
        };
        let empty_now = stat(0, 5);
        assert_eq!(
            decide(Some(&placeholder), &empty_now, never).unwrap(),
            Plan::Parse {
                from: 0,
                reason: ParseReason::Appended
            }
        );
    }

    #[test]
    fn the_hash_covers_the_last_complete_line_with_its_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = utf8_dir(&dir).join("f.jsonl");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"first\nsecond\npartial").unwrap();
        let expected: [u8; 32] = Sha256::digest(b"second\n").into();
        assert_eq!(last_line_hash(&path, 13).unwrap(), Some(expected));
        let first: [u8; 32] = Sha256::digest(b"first\n").into();
        assert_eq!(last_line_hash(&path, 6).unwrap(), Some(first));
        assert_ne!(
            last_line_hash(&path, 12).unwrap(),
            Some(expected),
            "an offset off the line boundary never matches"
        );
        assert_eq!(last_line_hash(&path, 0).unwrap(), None);
        assert_eq!(last_line_hash(&path, 999).unwrap(), None, "short file");
    }
}
