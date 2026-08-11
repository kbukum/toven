//! `toven-semver` — the pure semver toolkit.
//!
//! "semver is semver": the version *mechanism* with no policy, no git, and no
//! ecosystem knowledge, so any adapter or crate can reuse it. It wraps
//! [`rskit_version::semver`] with the three primitives Toven needs:
//!
//! - [`bump`] — the semver-increment matrix ([`next_version`], [`EffectiveLevel`]):
//!   advance a component, finalize a pending prerelease, or start/continue a
//!   prerelease train on a channel.
//! - [`tag`] — the release [`TagScheme`] codec that surrounds a version with
//!   fixed prefix/suffix text (`rust/core@1.2.3`, `cache/redis/v1.2.3`).
//! - [`select`] — [`latest_matching`], the max-semver selection over any
//!   [`Tagged`] items a [`TagScheme`] can parse.
//!
//! The *decision* of which bump to take, and any tag/baseline policy, lives in
//! the capability crates above this one; this crate only supplies the math.

pub mod bump;
pub mod select;
pub mod tag;

pub use bump::{EffectiveLevel, next_version};
pub use select::{Tagged, latest_matching};
pub use tag::TagScheme;
