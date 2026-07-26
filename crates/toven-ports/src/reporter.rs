//! Reporter — the synchronous, ordered observability **output port**.

use rskit_errors::AppResult;
use toven_model::Event;

/// A sink that renders the engine's typed [`Event`] stream.
///
/// This is an **output port** (the fat engine emits vocabulary; thin sinks
/// consume it), not an event bus: `emit` is called **in order on the engine
/// thread** — no pub/sub, no async reordering. Built-in sinks (Human, Jsonl)
/// and future ones (GH-annotations, `JUnit`) implement it without any engine
/// change.
pub trait Reporter: Send {
    /// Render one event. Called synchronously, in emission order.
    fn emit(&mut self, event: &Event) -> AppResult<()>;
}

/// Defers PLAN events until the caller knows whether planning succeeded.
///
/// On success, [`PlanReporter::commit`] emits the run-opening event followed by
/// every deferred PLAN event in its original order. On failure,
/// [`PlanReporter::abort`] emits only actionable diagnostics and discards
/// lifecycle framing for a run that never started.
pub struct PlanReporter<'a> {
    sink: &'a mut dyn Reporter,
    events: Vec<Event>,
}

impl<'a> PlanReporter<'a> {
    /// Wrap `sink` with an empty PLAN transaction.
    pub fn new(sink: &'a mut dyn Reporter) -> Self {
        Self {
            sink,
            events: Vec::new(),
        }
    }

    /// Commit a successful PLAN, preserving the established opening-first
    /// event order.
    pub fn commit(self, opening: &Event) -> AppResult<()> {
        self.sink.emit(opening)?;
        for event in &self.events {
            self.sink.emit(event)?;
        }
        Ok(())
    }

    /// Abort a failed PLAN while retaining diagnostics that explain the
    /// failure or repository state.
    pub fn abort(self) -> AppResult<()> {
        for event in &self.events {
            if matches!(event, Event::Warning { .. } | Event::FullActivation { .. }) {
                self.sink.emit(event)?;
            }
        }
        Ok(())
    }
}

impl Reporter for PlanReporter<'_> {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        self.events.push(event.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{PlanReporter, Reporter};
    use rskit_errors::AppResult;
    use toven_model::{Event, Phase};

    #[derive(Default)]
    struct RecordingReporter(Vec<Event>);

    impl Reporter for RecordingReporter {
        fn emit(&mut self, event: &Event) -> AppResult<()> {
            self.0.push(event.clone());
            Ok(())
        }
    }

    #[test]
    fn commit_opens_the_run_then_replays_plan_events_in_order() {
        let mut sink = RecordingReporter::default();
        let mut transaction = PlanReporter::new(&mut sink);
        transaction
            .emit(&Event::PhaseStarted {
                phase: Phase::Schedule,
            })
            .expect("buffer phase");
        transaction
            .emit(&Event::PlanPrepared { waves: 1, units: 2 })
            .expect("buffer plan");

        let opening = Event::RunStarted {
            run_id: "r1".into(),
            intent: "test".into(),
            project: "toven".into(),
        };
        transaction.commit(&opening).expect("commit");

        assert!(matches!(sink.0.first(), Some(Event::RunStarted { .. })));
        assert!(matches!(
            sink.0.get(1),
            Some(Event::PhaseStarted {
                phase: Phase::Schedule
            })
        ));
        assert!(matches!(
            sink.0.get(2),
            Some(Event::PlanPrepared { waves: 1, units: 2 })
        ));
    }

    #[test]
    fn abort_preserves_diagnostics_without_lifecycle_framing() {
        let mut sink = RecordingReporter::default();
        let mut transaction = PlanReporter::new(&mut sink);
        transaction
            .emit(&Event::PhaseStarted {
                phase: Phase::Configure,
            })
            .expect("buffer phase");
        transaction
            .emit(&Event::Warning {
                message: "go driver absent".into(),
            })
            .expect("buffer warning");

        transaction.abort().expect("abort");

        assert_eq!(sink.0.len(), 1);
        assert!(
            matches!(sink.0.first(), Some(Event::Warning { message }) if message == "go driver absent")
        );
    }
}
