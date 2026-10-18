//! Newline-delimited framing for the control channel.
//!
//! Split a stream with `into_split()` and give each half to a [`LineReader`] / [`LineWriter`],
//! so one task can read requests while another pushes events.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Longest accepted line, newline excluded.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Framing failure. After any error other than [`FrameError::Io`] the peer is misbehaving and the
/// connection should be dropped.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The line exceeds [`MAX_LINE_BYTES`].
    #[error("line exceeds {MAX_LINE_BYTES} bytes")]
    TooLong,
    /// The peer closed the stream in the middle of a line.
    #[error("stream closed mid-line")]
    Truncated,
    /// The line is not UTF-8.
    #[error("line is not valid UTF-8")]
    NotUtf8,
    /// The line to write contains `\n`, which would split it into two frames.
    #[error("line contains a newline")]
    EmbeddedNewline,
    /// Underlying transport error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Reads newline-terminated UTF-8 lines of at most [`MAX_LINE_BYTES`].
#[derive(Debug)]
pub struct LineReader<R> {
    inner: BufReader<R>,
    /// Bytes of the line in progress; kept across calls so `read_line` is cancel-safe.
    pending: Vec<u8>,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    /// Wraps `reader`.
    #[must_use]
    pub fn new(reader: R) -> Self {
        Self {
            inner: BufReader::new(reader),
            pending: Vec::new(),
        }
    }

    /// Next line without its terminator, or `None` on a clean end of stream.
    ///
    /// Cancel-safe: dropping the future loses no data.
    ///
    /// # Errors
    /// [`FrameError::TooLong`] before buffering more than the limit; [`FrameError::Truncated`] if
    /// the stream ends mid-line; [`FrameError::NotUtf8`]; [`FrameError::Io`].
    pub async fn read_line(&mut self) -> Result<Option<String>, FrameError> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            let newline = available.iter().position(|&b| b == b'\n');
            let take = newline.unwrap_or(available.len());
            if self.pending.len() + take > MAX_LINE_BYTES {
                return Err(FrameError::TooLong);
            }
            self.pending.extend_from_slice(&available[..take]);
            self.inner.consume(take + usize::from(newline.is_some()));
            if newline.is_some() {
                let bytes = std::mem::take(&mut self.pending);
                return String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| FrameError::NotUtf8);
            }
        }
    }
}

/// Writes newline-terminated lines.
#[derive(Debug)]
pub struct LineWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> LineWriter<W> {
    /// Wraps `writer`.
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self { inner: writer }
    }

    /// Writes `line` plus `\n` and flushes.
    ///
    /// # Errors
    /// [`FrameError::EmbeddedNewline`] or [`FrameError::TooLong`] (nothing is written);
    /// [`FrameError::Io`].
    pub async fn write_line(&mut self, line: &str) -> Result<(), FrameError> {
        if line.contains('\n') {
            return Err(FrameError::EmbeddedNewline);
        }
        if line.len() > MAX_LINE_BYTES {
            return Err(FrameError::TooLong);
        }
        let mut frame = Vec::with_capacity(line.len() + 1);
        frame.extend_from_slice(line.as_bytes());
        frame.push(b'\n');
        self.inner.write_all(&frame).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn round_trips_lines_and_reports_clean_eof() {
        let (a, b) = duplex(64);
        let mut writer = LineWriter::new(a);
        let mut reader = LineReader::new(b);
        writer.write_line(r#"{"a":1}"#).await.unwrap();
        writer.write_line("").await.unwrap();
        drop(writer);
        assert_eq!(
            reader.read_line().await.unwrap().as_deref(),
            Some(r#"{"a":1}"#)
        );
        assert_eq!(reader.read_line().await.unwrap().as_deref(), Some(""));
        assert_eq!(reader.read_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn oversized_line_is_rejected_without_buffering_it_all() {
        let mut data = vec![b'x'; MAX_LINE_BYTES + 1];
        data.push(b'\n');
        let mut reader = LineReader::new(data.as_slice());
        assert!(matches!(reader.read_line().await, Err(FrameError::TooLong)));
    }

    #[tokio::test]
    async fn line_of_exactly_the_limit_is_accepted() {
        let mut data = vec![b'x'; MAX_LINE_BYTES];
        data.push(b'\n');
        let mut reader = LineReader::new(data.as_slice());
        assert_eq!(
            reader.read_line().await.unwrap().map(|l| l.len()),
            Some(MAX_LINE_BYTES)
        );
    }

    #[tokio::test]
    async fn truncated_and_non_utf8_lines_are_rejected() {
        let mut reader = LineReader::new(&b"abc"[..]);
        assert!(matches!(
            reader.read_line().await,
            Err(FrameError::Truncated)
        ));
        let mut reader = LineReader::new(&b"\xff\n"[..]);
        assert!(matches!(reader.read_line().await, Err(FrameError::NotUtf8)));
    }

    #[tokio::test]
    async fn writer_rejects_embedded_newline_and_oversized_lines() {
        let (a, _b) = duplex(64);
        let mut writer = LineWriter::new(a);
        assert!(matches!(
            writer.write_line("a\nb").await,
            Err(FrameError::EmbeddedNewline)
        ));
        let big = "x".repeat(MAX_LINE_BYTES + 1);
        assert!(matches!(
            writer.write_line(&big).await,
            Err(FrameError::TooLong)
        ));
    }
}
