//! Field-merge helpers — resolving adapter default tasks and ecosystem release
//! config against user overrides.

mod coverage;
mod field;
mod release;

pub use coverage::merge_coverage;
pub use field::merge_task;
pub use release::merge_release;
