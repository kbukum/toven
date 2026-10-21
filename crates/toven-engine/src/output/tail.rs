//! [`TailBuffer`] — the bounded last-lines window kept for a quiet unit.
//!
//! A quiet run never prints a passing unit, so it only needs enough of each
//! unit's output to explain a failure. The window keeps the last `max_lines`
//! lines (each capped at [`MAX_LINE_BYTES`]) and counts what it dropped, so
//! memory per unit is bounded no matter how much the child prints. The full
//! output still lands in the unit's log file.

use std::collections::VecDeque;

use toven_model::{OutputStream, UnitOutput};

/// Longest single line kept in the window; longer lines are cut and marked.
pub(super) const MAX_LINE_BYTES: usize = 4 * 1024;

const CUT_MARKER: &[u8] = "…\n".as_bytes();

/// The last lines of one unit's output, in arrival order.
pub(super) struct TailBuffer {
    unit_id: String,
    max_lines: usize,
    lines: VecDeque<(OutputStream, Vec<u8>)>,
    /// The line still being written (no trailing newline yet).
    open: Option<(OutputStream, Vec<u8>)>,
    dropped_lines: usize,
    cr_stream: Option<OutputStream>,
}

impl TailBuffer {
    pub(super) fn new(unit_id: impl Into<String>, max_lines: usize) -> Self {
        Self {
            unit_id: unit_id.into(),
            max_lines: max_lines.max(1),
            lines: VecDeque::new(),
            open: None,
            dropped_lines: 0,
            cr_stream: None,
        }
    }

    /// Add one chunk, normalizing CR/CRLF line endings and evicting the oldest past the cap.
    pub(super) fn push(&mut self, chunk: &UnitOutput) {
        for piece in chunk
            .bytes
            .split_inclusive(|byte| matches!(byte, b'\r' | b'\n'))
        {
            let follows_cr = self.cr_stream.take() == Some(chunk.stream);
            if follows_cr && piece == b"\n" {
                continue;
            }
            let carriage_return = piece.last() == Some(&b'\r');
            let complete = carriage_return || piece.last() == Some(&b'\n');
            self.cr_stream = carriage_return.then_some(chunk.stream);
            let (stream, mut line) = match self.open.take() {
                Some((stream, line)) if stream == chunk.stream => (stream, line),
                Some((stream, mut line)) => {
                    // A different stream interrupted a partial line: end it as its own line.
                    if line.last() != Some(&b'\n') {
                        line.push(b'\n');
                    }
                    self.close_line((stream, line));
                    (chunk.stream, Vec::new())
                }
                None => (chunk.stream, Vec::new()),
            };
            if carriage_return {
                append_capped(&mut line, &piece[..piece.len() - 1]);
                append_capped(&mut line, b"\n");
            } else {
                append_capped(&mut line, piece);
            }
            if complete {
                self.close_line((stream, line));
            } else {
                self.open = Some((stream, line));
                if self.lines.len() == self.max_lines {
                    self.lines.pop_front();
                    self.dropped_lines = self.dropped_lines.saturating_add(1);
                }
            }
        }
    }

    /// Number of lines evicted from the front of the window.
    pub(super) const fn dropped_lines(&self) -> usize {
        self.dropped_lines
    }

    /// The kept lines (and any unterminated last line) as output chunks,
    /// oldest first.
    pub(super) fn chunks(&self) -> Vec<UnitOutput> {
        self.lines
            .iter()
            .chain(self.open.iter())
            .map(|(stream, bytes)| UnitOutput {
                unit_id: self.unit_id.clone(),
                stream: *stream,
                bytes: bytes.clone(),
            })
            .collect()
    }

    fn close_line(&mut self, line: (OutputStream, Vec<u8>)) {
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
            self.dropped_lines = self.dropped_lines.saturating_add(1);
        }
    }
}

fn append_capped(line: &mut Vec<u8>, piece: &[u8]) {
    if piece.is_empty() || line.len() > MAX_LINE_BYTES {
        return;
    }
    let content = piece.strip_suffix(b"\n").unwrap_or(piece);
    let room = MAX_LINE_BYTES - line.len();
    if content.len() <= room {
        line.extend_from_slice(piece);
    } else {
        line.extend_from_slice(&piece[..room]);
        line.extend_from_slice(CUT_MARKER);
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{OutputStream, UnitOutput};

    use super::{MAX_LINE_BYTES, TailBuffer};

    fn chunk(stream: OutputStream, bytes: &[u8]) -> UnitOutput {
        UnitOutput {
            unit_id: "u".into(),
            stream,
            bytes: bytes.to_vec(),
        }
    }

    fn text(chunks: &[UnitOutput]) -> String {
        chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(&chunk.bytes).into_owned())
            .collect()
    }

    #[test]
    fn keeps_only_the_last_lines_and_counts_the_rest() {
        let mut tail = TailBuffer::new("u", 2);
        tail.push(&chunk(OutputStream::Stdout, b"a\nb\nc\n"));
        tail.push(&chunk(OutputStream::Stdout, b"d\n"));
        assert_eq!(tail.dropped_lines(), 2);
        assert_eq!(text(&tail.chunks()), "c\nd\n");
    }

    #[test]
    fn joins_a_line_split_across_chunks() {
        let mut tail = TailBuffer::new("u", 5);
        tail.push(&chunk(OutputStream::Stderr, b"err"));
        tail.push(&chunk(OutputStream::Stderr, b"or: x\n"));
        let chunks = tail.chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].stream, OutputStream::Stderr);
        assert_eq!(text(&chunks), "error: x\n");
    }

    #[test]
    fn ends_a_partial_line_another_stream_interrupts() {
        let mut tail = TailBuffer::new("u", 5);
        tail.push(&chunk(OutputStream::Stdout, b"progress"));
        tail.push(&chunk(OutputStream::Stderr, b"error: x\n"));
        assert_eq!(text(&tail.chunks()), "progress\nerror: x\n");
    }

    #[test]
    fn keeps_an_unterminated_final_line() {
        let mut tail = TailBuffer::new("u", 5);
        tail.push(&chunk(OutputStream::Stdout, b"done"));
        assert_eq!(text(&tail.chunks()), "done");
    }

    #[test]
    fn unterminated_final_line_counts_toward_the_window() {
        let mut tail = TailBuffer::new("u", 2);
        tail.push(&chunk(OutputStream::Stdout, b"one\ntwo\nlast"));
        assert_eq!(text(&tail.chunks()), "two\nlast");
        assert_eq!(tail.dropped_lines(), 1);
    }

    #[test]
    fn carriage_return_progress_keeps_the_final_diagnostic() {
        let mut tail = TailBuffer::new("u", 2);
        for _ in 0..MAX_LINE_BYTES {
            tail.push(&chunk(OutputStream::Stderr, b"working\r"));
        }
        tail.push(&chunk(OutputStream::Stderr, b"error: failed\r"));
        assert_eq!(text(&tail.chunks()), "working\nerror: failed\n");
        assert_eq!(tail.dropped_lines(), MAX_LINE_BYTES - 1);
    }

    #[test]
    fn crlf_across_chunks_is_one_line_ending() {
        let mut tail = TailBuffer::new("u", 2);
        tail.push(&chunk(OutputStream::Stdout, b"one\r"));
        tail.push(&chunk(OutputStream::Stdout, b"\ntwo\r\nthree\r"));
        tail.push(&chunk(OutputStream::Stdout, b"\n"));
        assert_eq!(text(&tail.chunks()), "two\nthree\n");
        assert_eq!(tail.dropped_lines(), 1);
    }

    #[test]
    fn carriage_return_does_not_consume_another_streams_newline() {
        let mut tail = TailBuffer::new("u", 2);
        tail.push(&chunk(OutputStream::Stdout, b"progress\r"));
        tail.push(&chunk(OutputStream::Stderr, b"\n"));
        assert_eq!(text(&tail.chunks()), "progress\n\n");
    }

    #[test]
    fn caps_a_very_long_line() {
        let mut tail = TailBuffer::new("u", 5);
        tail.push(&chunk(
            OutputStream::Stdout,
            &vec![b'x'; MAX_LINE_BYTES * 3],
        ));
        let chunks = tail.chunks();
        assert!(chunks[0].bytes.len() <= MAX_LINE_BYTES + 4);
        assert!(text(&chunks).ends_with("…\n"));
    }

    #[test]
    fn a_full_length_line_ending_is_not_marked_as_truncated() {
        for ending in [b"\n".as_slice(), b"\r", b"\r\n"] {
            let mut tail = TailBuffer::new("u", 5);
            tail.push(&chunk(OutputStream::Stdout, &vec![b'x'; MAX_LINE_BYTES]));
            tail.push(&chunk(OutputStream::Stdout, ending));
            let chunks = tail.chunks();
            assert_eq!(chunks[0].bytes.len(), MAX_LINE_BYTES + 1);
            assert!(chunks[0].bytes.ends_with(b"x\n"));
        }
    }

    #[test]
    fn marks_a_line_that_fills_the_cap_before_its_next_chunk() {
        let mut tail = TailBuffer::new("u", 5);
        tail.push(&chunk(OutputStream::Stdout, &vec![b'x'; MAX_LINE_BYTES]));
        tail.push(&chunk(OutputStream::Stdout, b"tail\n"));
        let chunks = tail.chunks();
        assert_eq!(chunks[0].bytes.len(), MAX_LINE_BYTES + 4);
        assert!(text(&chunks).ends_with("…\n"));
    }
}
