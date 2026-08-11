//! The rskit-git-backed VCS mechanism: the one adapter, the change foundation,
//! and the per-repo reader-set fan-out.
//!
//! Grouped one level below the crate root so the adapter-internal compositions
//! (`changed` / `worktree` / `tags` / `commits` / `convert`) stay crate-private
//! helpers while the curated surface is re-exported from the crate root. See
//! the crate root for the public API.

mod adapter;
mod changed;
mod commits;
mod convert;
mod diff;
mod repo_set;
mod tags;
mod worktree;

pub use adapter::RskitGitVcs;
pub use diff::{resolve_range, resolve_range_optional};
pub use repo_set::{MemberPlacement, RepoGroup, VcsReaderSet, rebase_records};
