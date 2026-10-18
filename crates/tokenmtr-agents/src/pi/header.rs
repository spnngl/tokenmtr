//! The session header: the first line of every pi session file.
//!
//! The directory name of a pi session is a lossy encoding of its cwd, so the cwd is always read
//! from the header. The header is also the only line that names the session and its parent,
//! and a scan that resumes from a checkpoint never sees it. [`read`] therefore re-reads the first
//! line of the file on every parse call: one small positioned read, whatever the start offset,
//! which keeps resumed parsing identical to whole-file parsing without caching state between
//! calls.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tokenmtr_core::SessionId;

use crate::error::AgentError;

/// The newest session format version the parser knows. Pi itself reads headers up to 1 MiB.
pub const NEWEST_KNOWN_VERSION: u64 = 3;

/// The longest first line [`read`] accepts.
const MAX_HEADER_BYTES: u64 = 1 << 20;

/// `{type:"session", version?, id, timestamp, cwd, parentSession?}`. Version 1 files have no
/// `version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHeader {
    /// The session id.
    pub id: SessionId,
    /// The declared format version; `None` for version 1.
    pub version: Option<u64>,
    /// Where the session started. Absent in the oldest files.
    pub cwd: Option<Utf8PathBuf>,
    /// The session this one was forked or split from, derived from the parent's file name.
    pub parent_session: Option<SessionId>,
}

impl SessionHeader {
    /// Parses one line. `None` when it is not a session header with a non-blank id.
    #[must_use]
    pub fn parse(line: &[u8]) -> Option<Self> {
        let raw: RawHeader<'_> = serde_json::from_slice(line).ok()?;
        if raw.kind.as_deref() != Some("session") {
            return None;
        }
        Some(Self {
            id: SessionId::try_from(raw.id.as_deref()?).ok()?,
            version: raw.version,
            cwd: raw
                .cwd
                .as_deref()
                .filter(|cwd| !cwd.is_empty())
                .map(Utf8PathBuf::from),
            parent_session: raw
                .parent_session
                .as_deref()
                .and_then(|path| session_id_from_file_name(Utf8Path::new(path))),
        })
    }

    /// The declared version when it is newer than [`NEWEST_KNOWN_VERSION`].
    #[must_use]
    pub fn unknown_version(&self) -> Option<u64> {
        self.version
            .filter(|version| *version > NEWEST_KNOWN_VERSION)
    }
}

/// The session id in a pi file name, `<timestamp>_<session id>.jsonl`. The timestamp never holds
/// an underscore, the id may. `None` for names of any other shape.
#[must_use]
pub fn session_id_from_file_name(path: &Utf8Path) -> Option<SessionId> {
    let (_, id) = path.file_stem()?.split_once('_')?;
    SessionId::try_from(id).ok()
}

/// Reads the header of the file at `path`: `None` when the first line is not a complete header
/// (an empty or just-created file, another format, longer than 1 MiB).
pub fn read(path: &Utf8Path) -> Result<Option<SessionHeader>, AgentError> {
    let io_error = |source| AgentError::Io {
        path: path.to_owned(),
        source,
    };
    let file = File::open(path).map_err(io_error)?;
    let mut line = Vec::new();
    BufReader::new(file.take(MAX_HEADER_BYTES))
        .read_until(b'\n', &mut line)
        .map_err(io_error)?;
    if line.pop() != Some(b'\n') {
        return Ok(None);
    }
    Ok(SessionHeader::parse(&line))
}

#[derive(Deserialize)]
struct RawHeader<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    version: Option<u64>,
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    cwd: Option<Cow<'a, str>>,
    #[serde(rename = "parentSession", borrow)]
    parent_session: Option<Cow<'a, str>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v3_and_v1_headers() {
        let v3 = SessionHeader::parse(
            br#"{"type":"session","version":3,"id":"abc-1","timestamp":"t","cwd":"/w/app","parentSession":"/s/2026-01-01T00-00-00-000Z_par_ent.jsonl"}"#,
        );
        assert_eq!(
            v3,
            Some(SessionHeader {
                id: SessionId::try_from("abc-1")
                    .ok()
                    .unwrap_or_else(|| unreachable!()),
                version: Some(3),
                cwd: Some(Utf8PathBuf::from("/w/app")),
                parent_session: SessionId::try_from("par_ent").ok(),
            })
        );
        let v1 = SessionHeader::parse(br#"{"type":"session","id":"abc-1","timestamp":"t"}"#);
        assert_eq!(v1.as_ref().map(|h| h.version), Some(None));
        assert_eq!(v1.and_then(|h| h.cwd), None);
    }

    #[test]
    fn rejects_other_lines() {
        assert_eq!(
            SessionHeader::parse(br#"{"type":"message","id":"x"}"#),
            None
        );
        assert_eq!(
            SessionHeader::parse(br#"{"type":"session","id":" "}"#),
            None
        );
        assert_eq!(SessionHeader::parse(b"{"), None);
    }

    #[test]
    fn only_newer_versions_are_unknown() {
        let header = |version| SessionHeader {
            id: SessionId::try_from("a")
                .ok()
                .unwrap_or_else(|| unreachable!()),
            version,
            cwd: None,
            parent_session: None,
        };
        assert_eq!(header(None).unknown_version(), None);
        assert_eq!(header(Some(3)).unknown_version(), None);
        assert_eq!(header(Some(4)).unknown_version(), Some(4));
    }

    #[test]
    fn session_id_follows_the_first_underscore() {
        let id = |name| session_id_from_file_name(Utf8Path::new(name)).map(|id| id.to_string());
        assert_eq!(
            id("/s/2026-01-01T00-00-00-000Z_abc_def.jsonl").as_deref(),
            Some("abc_def")
        );
        assert_eq!(id("/s/custom.jsonl"), None);
    }
}
