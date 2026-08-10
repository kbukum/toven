//! The shared quiet [`Reporter`] for read-only projection verbs.
//!
//! Verbs whose stdout payload is a machine/human projection (the coverage
//! verdict table, the release projections) run their engine pass through this
//! reporter: it swallows progress events and surfaces only [`Event::Warning`]s
//! on stderr, so warn-and-skip diagnostics stay visible without polluting the
//! stdout projection.

use rskit_errors::AppResult;
use toven_model::Event;
use toven_ports::Reporter;

/// A [`Reporter`] that emits only warnings (on stderr), for verbs whose stdout
/// is a projection rather than a run log.
pub(crate) struct QuietReporter;

impl Reporter for QuietReporter {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        if let Event::Warning { message } = event {
            eprintln!("warning: {message}");
        }
        Ok(())
    }
}
