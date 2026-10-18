//! The shared line reader: positioned reads into a reused buffer, `memchr` line splitting
//! and a byte-needle pre-filter. Any agent source built on JSONL files uses it.
//!
//! - Only complete lines (ending in `\n`) are handed out. A half-written last line is not
//!   consumed: [`LineScan::new_offset`] stays before it.
//! - A line without any needle is skipped unparsed, but its bytes still count.
//! - No `mmap`: a file truncated while mapped raises `SIGBUS`.

use std::cell::RefCell;
use std::fs::File;
use std::io;

use camino::Utf8Path;
use memchr::memmem::Finder;
use sha2::{Digest as _, Sha256};

use crate::error::AgentError;
use crate::source::ByteOffset;

/// Size of the reused read buffer per thread.
pub const READ_BUFFER_BYTES: usize = 1 << 20;

/// Lines longer than this are dropped unparsed and counted in [`LineScan::oversized`].
pub const MAX_LINE_BYTES: usize = 128 << 20;

/// The key text that makes a line usage-bearing for drift accounting, in every agent.
pub const USAGE_NEEDLE: &[u8] = b"usage";

/// The last-line hash ([`line_hash`]) covers at most this many bytes; a longer line hashes its
/// tail.
pub const HASH_WINDOW: usize = 64 * 1024;

/// SHA-256 of the last line of `window`, the bytes just before an offset: from the newline that
/// precedes it (exclusive) to the end, so the line's own terminating `\n` is included. `None`
/// for an empty window.
///
/// Including the terminator makes the hash prove that the offset sits on a line boundary.
#[must_use]
pub fn line_hash(window: &[u8]) -> Option<[u8; 32]> {
    let (_, body) = window.split_last()?;
    let start = memchr::memrchr(b'\n', body).map_or(0, |at| at + 1);
    let window = &window[window.len().saturating_sub(HASH_WINDOW).max(start)..];
    Some(Sha256::digest(window).into())
}

thread_local! {
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The pre-filter: byte strings searched with `memchr::memmem`. A line passes when it contains
/// at least one of them.
#[derive(Debug, Clone)]
pub struct NeedleSet {
    finders: Option<Vec<Finder<'static>>>,
}

impl NeedleSet {
    /// Passes lines that contain any of `needles` (an agent's `line_needles`).
    #[must_use]
    pub fn new(needles: &'static [&'static [u8]]) -> Self {
        Self {
            finders: Some(needles.iter().map(Finder::new).collect()),
        }
    }

    /// Passes every line: the pre-filter is off.
    #[must_use]
    pub const fn all() -> Self {
        Self { finders: None }
    }

    /// Whether the line passes.
    #[must_use]
    pub fn matches(&self, line: &[u8]) -> bool {
        self.finders
            .as_ref()
            .is_none_or(|finders| finders.iter().any(|finder| finder.find(line).is_some()))
    }
}

/// What one [`read_lines`] call saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineScan {
    /// Offset after the last complete line.
    pub new_offset: ByteOffset,
    /// Complete lines read, including filtered and empty ones.
    pub lines: u64,
    /// Lines handed to the callback.
    pub passed: u64,
    /// Lines skipped by the pre-filter, plus empty lines.
    pub filtered: u64,
    /// Lines over [`MAX_LINE_BYTES`], dropped.
    pub oversized: u64,
    /// Of [`oversized`](Self::oversized), the lines that mention [`USAGE_NEEDLE`]. A mention
    /// split across two 128 MiB chunks is missed.
    pub oversized_usage: u64,
    /// [`line_hash`] of the last line consumed, computed from the bytes read so it describes the
    /// file as parsed. `None` when no line ended, or when the last one was oversized and its
    /// hashed tail is gone: hash the file then.
    pub last_line_hash: Option<[u8; 32]>,
}

/// Reads the complete lines of `path` from `start`, calling `on_line` (without the `\n`) for each
/// line that passes `needles`.
///
/// `start` must be a line boundary. The 1 MiB buffer is reused per thread and grows only for a
/// single line larger than the buffer.
pub fn read_lines(
    path: &Utf8Path,
    start: ByteOffset,
    needles: &NeedleSet,
    mut on_line: impl FnMut(&[u8]),
) -> Result<LineScan, AgentError> {
    let io_error = |source| AgentError::Io {
        path: path.to_owned(),
        source,
    };
    let file = File::open(path).map_err(io_error)?;
    let mut buf = BUFFER.take();
    if buf.len() < READ_BUFFER_BYTES {
        buf.resize(READ_BUFFER_BYTES, 0);
    }
    let scan = scan_file(&file, start, needles, &mut buf, &mut on_line);
    if buf.len() > READ_BUFFER_BYTES {
        buf.truncate(READ_BUFFER_BYTES);
        buf.shrink_to(READ_BUFFER_BYTES);
    }
    BUFFER.set(buf);
    scan.map_err(io_error)
}

fn scan_file(
    file: &File,
    start: ByteOffset,
    needles: &NeedleSet,
    buf: &mut Vec<u8>,
    on_line: &mut impl FnMut(&[u8]),
) -> io::Result<LineScan> {
    let mut scan = LineScan {
        new_offset: start,
        ..LineScan::default()
    };
    // File offset of `buf[0]`; `buf[..filled]` holds the unprocessed tail.
    let mut buf_start = start.get();
    let mut filled = 0_usize;
    // Inside a line longer than `MAX_LINE_BYTES`: drop bytes up to its newline.
    let mut discarding = false;
    // The dropped part of that line mentions usage.
    let mut discarded_usage = false;
    // The last consumed line, up to `HASH_WINDOW` bytes with its `\n`.
    let mut tail: Vec<u8> = Vec::new();
    let mut tail_whole = false;
    let usage = Finder::new(USAGE_NEEDLE);
    loop {
        if filled == buf.len() {
            // A full buffer holds no newline: one long line.
            if buf.len() >= MAX_LINE_BYTES {
                discarded_usage |= usage.find(&buf[..filled]).is_some();
                discarding = true;
                buf_start += filled as u64;
                filled = 0;
            } else {
                buf.resize((buf.len() * 2).min(MAX_LINE_BYTES), 0);
            }
        }
        let read = read_at(file, &mut buf[filled..], buf_start + filled as u64)?;
        if read == 0 {
            scan.last_line_hash = if tail_whole { line_hash(&tail) } else { None };
            return Ok(scan);
        }
        let end = filled + read;
        let mut line_start = 0;
        // Start and newline of the last line that ends in this chunk, and whether it was cut.
        let mut last: Option<(usize, usize, bool)> = None;
        while let Some(found) = memchr::memchr(b'\n', &buf[line_start..end]) {
            let line_end = line_start + found;
            scan.lines += 1;
            last = Some((line_start, line_end, discarding));
            if discarding {
                discarding = false;
                scan.oversized += 1;
                if discarded_usage || usage.find(&buf[line_start..line_end]).is_some() {
                    scan.oversized_usage += 1;
                }
                discarded_usage = false;
            } else {
                let line = &buf[line_start..line_end];
                if line.is_empty() || !needles.matches(line) {
                    scan.filtered += 1;
                } else {
                    scan.passed += 1;
                    on_line(line);
                }
            }
            line_start = line_end + 1;
            scan.new_offset = ByteOffset::new(buf_start + line_start as u64);
        }
        if let Some((start, newline, cut)) = last {
            let line = &buf[start..=newline];
            let kept = &line[line.len().saturating_sub(HASH_WINDOW)..];
            tail_whole = !cut || kept.len() == HASH_WINDOW;
            tail.clear();
            tail.extend_from_slice(kept);
        }
        buf.copy_within(line_start..end, 0);
        buf_start += line_start as u64;
        filled = end - line_start;
    }
}

fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    loop {
        #[cfg(unix)]
        let read = std::os::unix::fs::FileExt::read_at(file, buf, offset);
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(file, buf, offset);
        match read {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const NEEDLES: &[&[u8]] = &[b"usage", b"tool_use"];

    fn scan_text(
        text: &[u8],
        start: u64,
        needles: &NeedleSet,
    ) -> Result<(Vec<String>, LineScan), Box<dyn std::error::Error>> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(text)?;
        let path = Utf8Path::from_path(file.path()).ok_or("temp path is not UTF-8")?;
        let mut lines = Vec::new();
        let scan = read_lines(path, ByteOffset::new(start), needles, |line| {
            lines.push(String::from_utf8_lossy(line).into_owned());
        })?;
        Ok((lines, scan))
    }

    #[test]
    fn half_written_last_line_is_not_consumed() -> TestResult {
        let (lines, scan) = scan_text(b"usage 1\nusage 2\nusage 3 par", 0, &NeedleSet::all())?;
        assert_eq!(lines, ["usage 1", "usage 2"]);
        assert_eq!(scan.new_offset.get(), 16);
        Ok(())
    }

    #[test]
    fn needle_less_lines_are_skipped_but_their_bytes_count() -> TestResult {
        let needles = NeedleSet::new(NEEDLES);
        let (lines, scan) = scan_text(b"hello\n\nusage 1\nbye\n", 0, &needles)?;
        assert_eq!(lines, ["usage 1"]);
        assert_eq!((scan.lines, scan.passed, scan.filtered), (4, 1, 3));
        assert_eq!(scan.new_offset.get(), 19);
        Ok(())
    }

    #[test]
    fn reads_from_a_mid_file_offset() -> TestResult {
        let (lines, scan) = scan_text(b"aaa\nbbb\nccc\n", 4, &NeedleSet::all())?;
        assert_eq!(lines, ["bbb", "ccc"]);
        assert_eq!(scan.new_offset.get(), 12);
        Ok(())
    }

    #[test]
    fn a_line_larger_than_the_buffer_is_read_whole() -> TestResult {
        let long = "x".repeat(READ_BUFFER_BYTES * 2 + 17);
        let text = format!("short\n{long}\nend\n");
        let (lines, scan) = scan_text(text.as_bytes(), 0, &NeedleSet::all())?;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].len(), long.len());
        assert_eq!(scan.new_offset.get(), text.len() as u64);
        Ok(())
    }

    #[test]
    fn the_scan_hashes_the_last_consumed_line_with_its_newline() -> TestResult {
        let (_, scan) = scan_text(b"first\nsecond\npartial", 0, &NeedleSet::all())?;
        let expected: [u8; 32] = Sha256::digest(b"second\n").into();
        assert_eq!(scan.last_line_hash, Some(expected));
        let (_, nothing) = scan_text(b"partial", 0, &NeedleSet::all())?;
        assert_eq!(nothing.last_line_hash, None);
        Ok(())
    }

    #[test]
    fn line_hash_takes_the_last_line_and_proves_the_boundary() {
        let hash = |bytes: &[u8]| -> [u8; 32] { Sha256::digest(bytes).into() };
        assert_eq!(line_hash(b"a\nbc\n"), Some(hash(b"bc\n")));
        assert_eq!(line_hash(b"a\nbc"), Some(hash(b"bc")));
        assert_ne!(line_hash(b"a\nbc\n"), line_hash(b"a\n\nbc"));
        assert_eq!(line_hash(b""), None);
        let long = vec![b'x'; HASH_WINDOW * 2];
        assert_eq!(line_hash(&long), Some(hash(&long[HASH_WINDOW..])));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let result = read_lines(
            Utf8Path::new("/nonexistent/tokenmtr.jsonl"),
            ByteOffset::ZERO,
            &NeedleSet::all(),
            |_| {},
        );
        assert!(matches!(result, Err(AgentError::Io { .. })));
    }
}
