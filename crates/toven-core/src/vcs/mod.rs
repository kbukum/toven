//! The engine-owned baseline policy over the git seam.
//!
//! The git *mechanism* — the one rskit-git-backed adapter, the change
//! foundation, and the per-repo reader-set fan-out — lives in the focused
//! [`toven-vcs`](../../toven_vcs/index.html) crate behind the
//! [`VcsReader`](toven_ports::VcsReader) / [`VcsWriter`](toven_ports::VcsWriter)
//! ports. This module retains only the pure baseline *policy* the engine owns:
//! resolving CLI flags + `[project].base_ref` into a typed
//! [`BaselineSpec`](toven_ports::BaselineSpec).
//!
//! ## Surface
//! - [`BaselineStrategy`] — the engine-owned named policy resolving CLI flags +
//!   `[project].base_ref` into a typed
//!   [`BaselineSpec`](toven_ports::BaselineSpec).

mod baseline;

pub use baseline::{BaselineFlags, BaselineStrategy};
