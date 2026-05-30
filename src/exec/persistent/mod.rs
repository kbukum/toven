//! Persistent execution-unit integration.

mod command;
mod error;
mod lifecycle;
mod output;
mod readiness;
mod tests;

pub(super) use lifecycle::{
    PersistentProcess, PersistentRun, start_persistent_execution_unit_with_output,
};
