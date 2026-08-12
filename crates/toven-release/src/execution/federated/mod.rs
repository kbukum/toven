//! Cross-repo release planning and per-member APPLY sharding.
//!
//! The release plan remains one federated plan, but history mutations are
//! scoped to each member repo: every member gets its own clean-tree guardrail,
//! release commit, tags, and optional push. Publishing is delayed until after
//! the member commits so registry work still follows the federated publish
//! order.

mod apply;
mod bump;
mod hooks;
mod restore;
#[cfg(test)]
mod tests;

#[allow(clippy::redundant_pub_crate)]
pub(crate) use apply::release_apply_by_member;
#[allow(clippy::redundant_pub_crate)]
pub(crate) use bump::release_bump_by_member;
