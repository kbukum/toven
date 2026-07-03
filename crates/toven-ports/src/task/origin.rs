//! Provenance of a resolved task.
//!
//! The origin vocabulary is owned by [`toven-model`](toven_model) (it rides on the
//! plan artifact's [`ExecutionUnit`](toven_model::ExecutionUnit)); this module
//! re-exports it so the adapter-facing [`Task`](super::Task) and the plan output
//! speak one type.

pub use toven_model::TaskOrigin;
