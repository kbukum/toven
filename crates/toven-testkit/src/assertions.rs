//! Assertion helpers: `rskit-testutil`'s `AppResult` assertions plus
//! Toven-domain [`Event`] assertions for the
//! [`RecordingReporter`](crate::doubles::RecordingReporter).
//!
//! The `AppResult` assertions are re-exported (canonical owner: `rskit-testutil`);
//! only the Toven-shaped event assertions are added here.

use toven_model::Event;

pub use rskit_testutil::{assert_err_code, assert_ok};

/// Find the first emitted event matching `predicate`, if any.
pub fn find_event(events: &[Event], predicate: impl Fn(&Event) -> bool) -> Option<&Event> {
    events.iter().find(|event| predicate(event))
}

/// Assert that at least one emitted event matches `predicate`.
#[track_caller]
pub fn assert_emitted(events: &[Event], predicate: impl Fn(&Event) -> bool) {
    assert!(
        events.iter().any(predicate),
        "expected an event matching the predicate, got: {events:?}",
    );
}

/// Assert the emitted events contain `expected` as an **ordered subsequence**.
///
/// Intervening events are allowed; the relative order of `expected` must hold.
/// This is the common shape for engine flow tests that pin lifecycle order
/// (e.g. `PlanPrepared` → `UnitStarted` → `UnitFinished`) without over-asserting
/// on every event.
#[track_caller]
pub fn assert_event_sequence(events: &[Event], expected: &[Event]) {
    let mut remaining = expected.iter();
    let mut next = remaining.next();
    for event in events {
        if Some(event) == next {
            next = remaining.next();
        }
    }
    assert!(
        next.is_none(),
        "expected ordered subsequence {expected:?} within {events:?}",
    );
}

#[cfg(test)]
mod tests {
    use toven_model::{Event, UnitStatus};

    use super::{assert_emitted, assert_event_sequence, find_event};

    fn sample() -> Vec<Event> {
        vec![
            Event::PlanPrepared { waves: 1, units: 1 },
            Event::UnitStarted {
                unit_id: "u1".into(),
            },
            Event::UnitFinished {
                unit_id: "u1".into(),
                status: UnitStatus::Succeeded,
            },
        ]
    }

    #[test]
    fn finds_and_asserts_present_event() {
        let events = sample();
        assert!(find_event(&events, |e| matches!(e, Event::PlanPrepared { .. })).is_some());
        assert_emitted(&events, |e| matches!(e, Event::UnitStarted { .. }));
    }

    #[test]
    fn ordered_subsequence_holds_with_gaps() {
        let events = sample();
        assert_event_sequence(
            &events,
            &[
                Event::PlanPrepared { waves: 1, units: 1 },
                Event::UnitFinished {
                    unit_id: "u1".into(),
                    status: UnitStatus::Succeeded,
                },
            ],
        );
    }

    #[test]
    #[should_panic(expected = "ordered subsequence")]
    fn ordered_subsequence_fails_when_out_of_order() {
        let events = sample();
        assert_event_sequence(
            &events,
            &[
                Event::UnitFinished {
                    unit_id: "u1".into(),
                    status: UnitStatus::Succeeded,
                },
                Event::PlanPrepared { waves: 1, units: 1 },
            ],
        );
    }
}
