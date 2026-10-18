//! The shared line reader (D48): positioned reads into a reused buffer, `memchr` line splitting
//! and a byte-needle pre-filter. Any agent source built on JSONL files uses it.
//!
//! - Only complete lines (ending in `\n`) are handed out. A half-written last line is not
//!   consumed: [`LineScan::new_offset`] stays before it (D15).
//! - A line without any needle is skipped unparsed, but its bytes still count (D42).
//! - No `mmap`: a file truncated while mapped raises `SIGBUS`.

use std::cell::RefCell;
use std::fs::File;
use std::io;

use camino::Utf8Path;
use memchr::memmem::Finder;

use crate::error::AgentError;
use crate::source::ByteOffset;

/// Size of the reused read buffer per thread.
pub const READ_BUFFER_BYTES: usize = 1 << 20;

/// Lines longer than this are dropped unparsed and counted in [`LineScan::oversized`].
pub const MAX_LINE_BYTES: usize = 128 << 20;

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
    loop {
        if filled == buf.len() {
            // A full buffer holds no newline: one long line.
            if buf.len() >= MAX_LINE_BYTES {
                discarding = true;
                buf_start += filled as u64;
                filled = 0;
            } else {
                buf.resize((buf.len() * 2).min(MAX_LINE_BYTES), 0);
            }
        }
        let read = read_at(file, &mut buf[filled..], buf_start + filled as u64)?;
        if read == 0 {
            return Ok(scan);
        }
        let end = filled + read;
        let mut line_start = 0;
        while let Some(found) = memchr::memchr(b'\n', &buf[line_start..end]) {
            let line_end = line_start + found;
            scan.lines += 1;
            if discarding {
                discarding = false;
                scan.oversized += 1;
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
