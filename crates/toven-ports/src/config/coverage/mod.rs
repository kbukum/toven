//! Coverage sub-config — the declarative `coverage.*` gating surface and its
//! vocabulary.
//!
//! [`CoverageConfig`] is the whole `[…coverage]` block, shared by the ecosystem
//! default, the per-module override, and (via [`CoverageProfile`]) an elevated
//! profile; the sibling modules hold the field vocabulary it composes
//! ([`CoverageThresholds`], [`Enforcement`]).

mod config;
mod enforcement;
mod profile;
mod thresholds;

pub use config::CoverageConfig;
pub use enforcement::Enforcement;
pub use profile::CoverageProfile;
pub use thresholds::CoverageThresholds;
