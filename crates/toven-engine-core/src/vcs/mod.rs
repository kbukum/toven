//! The VCS adapter + engine-owned baseline policy.
//!
//! The implementation side of the single git seam whose trait ports
//! ([`VcsReader`](toven_ports::VcsReader) /
//! [`VcsWriter`](toven_ports::VcsWriter)) live in `toven-ports`. The engine and
//! every flow depend on the *traits*; this module is the one rskit-git-backed
//! adapter behind them, plus the pure baseline policy and per-repo fan-out the
//! engine owns.
//!
//! ## Surface
//! - [`RskitGitVcs`] — the single rskit-git-backed adapter implementing both
//!   port halves.
//! - [`BaselineStrategy`] — the engine-owned named policy resolving CLI flags +
//!   `[project].base_ref` into a typed
//!   [`BaselineSpec`](toven_ports::BaselineSpec).
//! - [`resolve_range`] / [`resolve_range_optional`] — the reusable change
//!   foundation resolving a [`DiffRange`](toven_ports::DiffRange) of two
//!   endpoints onto the git seam.
//! - [`latest_matching`] — the shared max-semver tag selection primitive.
//! - [`VcsReaderSet`] — per-repo dedup + open + the pure record-to-workspace
//!   fan-out ([`rebase_records`]).
//!
//! The `changed` / `worktree` / `tags` / `convert` submodules are
//! adapter-internal compositions of rskit-git primitives.

mod adapter;
mod baseline;
mod changed;
mod commits;
mod convert;
mod diff;
mod repo_set;
mod tags;
mod worktree;

pub use adapter::RskitGitVcs;
pub use baseline::{BaselineFlags, BaselineStrategy};
pub use diff::{resolve_range, resolve_range_optional};
pub use repo_set::{MemberPlacement, RepoGroup, VcsReaderSet, rebase_records};
pub use tags::latest_matching;
