//! Release versioning: bump planning and the semver-increment matrix, the
//! standalone `release bump` verb, change detection, and Conventional-Commit
//! changelog generation.

#[allow(clippy::redundant_pub_crate)]
pub(crate) mod bump;
mod bump_verb;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod change;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod changelog;
mod conventional;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod strategy;

pub use bump_verb::{BumpModuleOutcome, BumpOptions, BumpReport, release_bump};
