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
        let line = serde_json::to_string(event).map_err(AppError::internal)?;
        self.writer
            .write_all(line.as_bytes())
            .map_err(AppError::internal)?;
        self.writer.write_all(b"\n").map_err(AppError::internal)?;
        // Flush each line so machine consumers tailing a pipe (non-TTY, where stdout is
        // block-buffered) see every Event immediately.
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
                exit_code: None,
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

    #[test]
    fn new_domain_events_are_one_record_each_and_round_trip() {
        use toven_model::{CoverageMeasurement, CoverageMetric, CoverageVerdict};

        let events = vec![
            Event::ModuleReleaseExamining {
                module: "core".into(),
            },
            Event::ModuleReleaseResolved {
                module: "core".into(),
                current_version: "1.2.0".into(),
                planned_version: Some("1.3.0".into()),
                level: "minor".into(),
                reason: "changed".into(),
                tag: Some("core-v1.3.0".into()),
                publication: Some("publish".into()),
                up_to_date: false,
            },
            Event::ModuleReleaseStaged {
                module: "core".into(),
                new_version: "1.3.0".into(),
                manifests: vec!["crates/core/Cargo.toml".into()],
                changelog: None,
                tag: Some("core-v1.3.0".into()),
            },
            Event::ModuleCoverageFinished {
                module: "core".into(),
                measurements: vec![CoverageMeasurement {
                    metric: CoverageMetric::Line,
                    measured: 9537,
                    threshold: Some(9000),
                    met: true,
                }],
                verdict: CoverageVerdict::Passed,
            },
        ];
        let output = render(&events);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), events.len(), "one record per event");
        for (line, event) in lines.iter().zip(&events) {
            let back: Event = serde_json::from_str(line).expect("parse line");
            assert_eq!(&back, event);
        }
        assert!(lines[0].contains(r#""event":"module-release-examining""#));
        assert!(lines[1].contains(r#""event":"module-release-resolved""#));
        assert!(lines[2].contains(r#""event":"module-release-staged""#));
        assert!(lines[3].contains(r#""event":"module-coverage-finished""#));
    }
}
