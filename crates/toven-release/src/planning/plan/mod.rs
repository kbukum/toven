//! Release PLAN tail orchestration.

mod changelog_required;
mod spine;
mod targets;
#[cfg(test)]
mod tests;
mod validation;

#[allow(clippy::redundant_pub_crate)]
pub(crate) use spine::plan_with_context;
pub use spine::release_plan;
#[allow(clippy::redundant_pub_crate)]
pub(crate) use targets::{release_targets, resolve_release_settings};
