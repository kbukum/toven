//! Reporter — the synchronous, ordered observability **output port**.

use rskit_errors::AppResult;
use toven_model::Event;

/// A sink that renders the engine's typed [`Event`] stream.
///
/// This is an **output port** (the fat engine emits vocabulary; thin sinks
/// consume it), not an event bus: `emit` is called **in order on the engine
/// thread** — no pub/sub, no async reordering. Built-in sinks (Human, Jsonl) and
/// future ones (GH-annotations, `JUnit`) implement it without any engine change.
pub trait Reporter: Send {
    /// Render one event. Called synchronously, in emission order.
    fn emit(&mut self, event: &Event) -> AppResult<()>;
}
