//! Release sub-config — the declarative `release.*` surface and its vocabulary.
//!
//! [`ReleaseConfig`] is the whole `[…release]` block, shared by the ecosystem
//! default and the per-module override; the sibling modules hold the field
//! vocabulary it composes ([`BumpLevel`]/[`DependentVersion`],
//! [`PrereleaseConfig`], [`ChangelogConfig`], [`SignConfig`], [`HooksConfig`]).

mod changelog;
mod config;
mod hooks;
mod host;
mod policy;
mod prerelease;
mod signing;

pub use changelog::ChangelogConfig;
pub use config::ReleaseConfig;
pub use hooks::HooksConfig;
pub use host::HostConfig;
pub use policy::{BumpLevel, DependentVersion};
pub use prerelease::PrereleaseConfig;
pub use signing::SignConfig;
