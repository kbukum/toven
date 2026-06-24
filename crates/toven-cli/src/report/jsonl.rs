//! [`JsonlReporter`] — the machine-parseable Event-stream sink.

use std::io::{self, Write};

use rskit_errors::{AppError, AppResult};
use toven_model::Event;
use toven_ports::Reporter;

/// A [`Reporter`] that writes each [`Event`] as one JSON object per line.
///
/// The newline-delimited stream stays machine-parseable on stdout; raw child
/// output travels on the separate per-unit channel (attributed to stderr), so
/// the two never interleave. Generic over the writer for testability;
/// [`JsonlReporter::stdout`] binds the process stdout.
pub struct JsonlReporter<W: Write> {
    writer: W,
}

impl<W: Write> JsonlReporter<W> {
    /// Create a reporter that serializes events to `writer`.
    pub const fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Consume the reporter and recover the underlying writer.
    ///
    /// Test-only: the production stdout reporter is write-only; recovering the
    /// writer exists solely so unit tests can assert the serialized bytes.
    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl JsonlReporter<io::Stdout> {
    /// Create a reporter writing newline-delimited JSON to process stdout.
    #[must_use]
    pub fn stdout() -> Self {
        Self::new(io::stdout())
    }
}

impl<W: Write + Send> Reporter for JsonlReporter<W> {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        serde_json::to_writer(&mut self.writer, event).map_err(AppError::internal)?;
        self.writer.write_all(b"\n").map_err(AppError::internal)?;
        // Flush each line so machine consumers tailing a pipe (non-TTY, where
        // stdout is block-buffered) see every Event immediately.
        self.writer.flush().map_err(AppError::internal)
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{Event, Phase, RunStats, UnitStatus};
    use toven_ports::Reporter;

    use super::JsonlReporter;

    fn render(events: &[Event]) -> String {
        let mut reporter = JsonlReporter::new(Vec::new());
        for event in events {
            reporter.emit(event).expect("emit");
        }
        String::from_utf8(reporter.into_inner()).expect("utf8")
    }

    #[test]
    fn each_event_is_one_round_trippable_json_line() {
        let events = vec![
            Event::RunStarted {
                run_id: "r1".into(),
                intent: "test".into(),
                project: "toven".into(),
            },
            Event::PhaseStarted {
                phase: Phase::Discover,
            },
            Event::UnitFinished {
                unit_id: "rust:errors#test".into(),
                status: UnitStatus::Succeeded,
            },
            Event::RunFinished {
                summary: RunStats::new(1),
            },
        ];
        let output = render(&events);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), events.len());
        for (line, event) in lines.iter().zip(&events) {
            let back: Event = serde_json::from_str(line).expect("parse line");
            assert_eq!(&back, event);
        }
    }

    #[test]
    fn tag_field_names_the_variant() {
        let output = render(&[Event::PlanPrepared { waves: 2, units: 5 }]);
        assert!(
            output.contains(r#""event":"plan-prepared""#),
            "got {output}"
        );
        assert!(output.ends_with('\n'));
    }
}
