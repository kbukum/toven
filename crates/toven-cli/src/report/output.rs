//! [`WriterRawSink`] — the terminal-bound adapter for the engine's raw-output
//! channel.
//!
//! The engine's `UnitOutputChannel`
//! owns the buffer-normal / live-persistent *policy* and never prints; this CLI
//! adapter is where the bytes actually land. It implements the
//! [`RawOutputSink`](toven_ports::RawOutputSink) port: a normal unit's buffered
//! output arrives as one or more [`block`](toven_ports::RawOutputSink::block)
//! calls (one on finish, plus an extra block whenever the unit spills past the
//! channel's buffer cap), each rendered under a `==> <unit_id>` header, and
//! persistent units stream through
//! [`live`](toven_ports::RawOutputSink::live) as they arrive. Raw bytes are
//! written verbatim (never interpreted as UTF-8) so build output is preserved;
//! [`WriterRawSink::stderr`] keeps them off the Jsonl Event stream on stdout.

use std::io::{self, Write};

use rskit_errors::{AppError, AppResult};
use toven_model::UnitOutput;
use toven_ports::RawOutputSink;

/// Renders the engine's per-unit raw-output channel to a writer.
///
/// Generic over the writer for testability; [`WriterRawSink::stderr`] binds the
/// process stderr (the default attribution target so the Jsonl Event stream on
/// stdout stays clean).
pub struct WriterRawSink<W: Write> {
    writer: W,
}

impl<W: Write> WriterRawSink<W> {
    /// Create a sink that renders raw output to `writer`.
    pub const fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Consume the sink and recover the underlying writer.
    ///
    /// Test-only: the production stderr sink is write-only; recovering the
    /// writer exists solely so unit tests can assert the rendered bytes.
    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl WriterRawSink<io::Stderr> {
    /// Create a sink that renders raw output to process stderr.
    #[must_use]
    pub fn stderr() -> Self {
        Self::new(io::stderr())
    }
}

impl<W: Write + Send> RawOutputSink for WriterRawSink<W> {
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()> {
        self.writer
            .write_all(&chunk.bytes)
            .map_err(AppError::internal)?;
        // Flush each live chunk so tailing a persistent unit stays responsive
        // even when the writer is redirected through a buffer.
        self.writer.flush().map_err(AppError::internal)
    }

    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()> {
        writeln!(self.writer, "==> {unit_id}").map_err(AppError::internal)?;
        for chunk in chunks {
            self.writer
                .write_all(&chunk.bytes)
                .map_err(AppError::internal)?;
        }
        // Flush once at the end of the block so a completed unit's output appears
        // promptly on redirected/buffered output, without flushing per chunk.
        self.writer.flush().map_err(AppError::internal)
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{OutputStream, UnitOutput};
    use toven_ports::RawOutputSink;

    use super::WriterRawSink;

    fn chunk(unit: &str, bytes: &[u8]) -> UnitOutput {
        UnitOutput {
            unit_id: unit.into(),
            stream: OutputStream::Stdout,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn block_renders_one_labeled_group() {
        let mut sink = WriterRawSink::new(Vec::new());
        sink.block("u1", &[chunk("u1", b"line a\n"), chunk("u1", b"line b\n")])
            .expect("block");
        let output = String::from_utf8(sink.into_inner()).expect("utf8");
        assert_eq!(output, "==> u1\nline a\nline b\n");
    }

    #[test]
    fn live_streams_bytes_without_a_header() {
        let mut sink = WriterRawSink::new(Vec::new());
        sink.live(&chunk("srv", b"serving\n")).expect("live");
        sink.live(&chunk("srv", b"ready\n")).expect("live");
        let output = String::from_utf8(sink.into_inner()).expect("utf8");
        assert_eq!(output, "serving\nready\n");
    }

    #[test]
    fn raw_bytes_are_written_verbatim() {
        let mut sink = WriterRawSink::new(Vec::new());
        sink.block("u1", &[chunk("u1", &[0xff, 0x00, 0x42])])
            .expect("block");
        let bytes = sink.into_inner();
        assert_eq!(&bytes[bytes.len() - 3..], &[0xff, 0x00, 0x42]);
    }
}
