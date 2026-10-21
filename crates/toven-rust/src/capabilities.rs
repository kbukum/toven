//! The module capabilities the Rust adapter reports during discovery and
//! requires in the tasks it renders.

use rskit_errors::AppResult;
use toven_model::Capability;

/// The capability of a crate with a `bin` target `cargo run -p` can launch.
pub(crate) fn bin() -> AppResult<Capability> {
    Capability::new("bin")
}
