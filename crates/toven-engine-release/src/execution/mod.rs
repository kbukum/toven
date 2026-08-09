//! Release APPLY execution: the single-repo release transaction, the shared
//! bump-phase mutation prefix, and the cross-repo federated sharding that drives
//! each member's APPLY.

#[allow(clippy::redundant_pub_crate)]
pub(crate) mod apply;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod federated;
mod mutate;
mod version_sync;

pub use apply::{ReleaseApplyOptions, release_apply};
