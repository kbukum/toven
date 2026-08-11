//! `toven-vcs` — the focused git-mechanism crate.
//!
//! Git's single owner, the way [`toven-exec`](../toven_exec/index.html) is
//! process's: the one rskit-git-backed adapter behind the
//! [`VcsReader`](toven_ports::VcsReader) / [`VcsWriter`](toven_ports::VcsWriter)
//! port halves, the reusable change foundation, and the per-repo reader-set
//! fan-out. Pure git mechanism only — baseline *policy* stays engine-owned in
//! `toven-core`.
//!
//! ## Surface
//! - [`RskitGitVcs`] — the single rskit-git-backed adapter implementing both
//!   port halves.
//! - [`resolve_range`] / [`resolve_range_optional`] — the reusable change
//!   foundation resolving a [`DiffRange`](toven_ports::DiffRange) of two
//!   endpoints onto the git seam.
//! - [`VcsReaderSet`] — per-repo dedup + open + the pure record-to-workspace
//!   fan-out ([`rebase_records`]), exposing [`RepoGroup`] / [`MemberPlacement`].
//!
//! The `changed` / `worktree` / `tags` / `commits` / `convert` submodules are
//! adapter-internal compositions of rskit-git primitives.
#![warn(missing_docs)]

mod git;

pub use git::{
    MemberPlacement, RepoGroup, RskitGitVcs, VcsReaderSet, rebase_records, resolve_range,
    resolve_range_optional,
};
