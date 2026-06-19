//! [`RecordingReporter`] — a [`Reporter`] that captures the emitted [`Event`]
//! sequence so tests can assert on ordering and content.

use rskit_errors::AppResult;
use toven_model::Event;
use toven_ports::Reporter;

/// A [`Reporter`] that records every emitted [`Event`] in order.
///
/// `emit` never fails; tests inspect [`RecordingReporter::events`] (or the
/// [`assert_event_sequence`](crate::assertions::assert_event_sequence) /
/// [`assert_emitted`](crate::assertions::assert_emitted) helpers) after driving
/// the engine.
#[derive(Debug, Default)]
pub struct RecordingReporter {
    events: Vec<Event>,
}

impl RecordingReporter {
    /// Construct an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The captured events, in emission order.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Number of captured events.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether no events have been captured yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Drop all captured events.
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

impl Reporter for RecordingReporter {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        self.events.push(event.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use toven_model::Event;
    use toven_ports::Reporter;

    use super::RecordingReporter;

    #[test]
    fn records_emitted_events_in_order() {
        let mut reporter = RecordingReporter::new();
        assert!(reporter.is_empty());

        reporter
            .emit(&Event::PlanPrepared { waves: 1, units: 2 })
            .expect("emits");
        reporter
            .emit(&Event::UnitStarted {
                unit_id: "u1".into(),
            })
            .expect("emits");

        assert_eq!(reporter.len(), 2);
        assert_eq!(
            reporter.events()[0],
            Event::PlanPrepared { waves: 1, units: 2 }
        );
    }
}
