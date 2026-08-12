//! Release versioning: entry assembly over the pure `toven-version` decision,
//! the standalone `release bump` verb, and change detection. The bump-policy
//! vocabulary, the semver-increment matrix, baseline anchoring, and
//! Conventional-Commit changelog generation live in `toven-version`.

#[allow(clippy::redundant_pub_crate)]
pub(crate) mod baseline;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod bump;
mod bump_verb;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod change;

pub use bump_verb::{BumpModuleOutcome, BumpOptions, BumpReport, release_bump};
