//! Core crate of the umbrella-registry sample workspace, built on `kit-util`.

/// The core entry point, composed from the leaf utility.
#[must_use]
pub fn core() -> u32 {
    kit_util::util()
}
