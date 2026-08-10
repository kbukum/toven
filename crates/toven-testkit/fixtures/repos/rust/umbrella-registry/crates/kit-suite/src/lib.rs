//! Umbrella facade crate re-exposing the toolkit through one aggregate.

/// The facade entry point, delegating to `kit-core`.
#[must_use]
pub fn suite() -> u32 {
    kit_core::core()
}
