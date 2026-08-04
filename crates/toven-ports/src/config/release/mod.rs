//! Release sub-config — the declarative `release.*` surface and its vocabulary.
//!
//! [`ReleaseConfig`] is the whole `[…release]` block, shared by the ecosystem
//! default and the per-module override; the sibling modules hold the field
//! vocabulary it composes ([`BumpLevel`]/[`DependentVersion`],
//! [`PrereleaseConfig`], [`ChangelogConfig`], [`SignConfig`],
//! [`PhasesConfig`] the per-phase native-or-delegated backing). Pre/post hooks
//! reuse the shared, verb-agnostic [`HooksConfig`](crate::config::HooksConfig).

mod changelog;
mod config;
mod host;
mod phases;
mod policy;
mod prerelease;
mod publication;
mod signing;

pub use changelog::ChangelogConfig;
pub use config::ReleaseConfig;
pub use host::HostConfig;
pub use phases::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};
pub use policy::{BumpLevel, DependentVersion};
pub use prerelease::PrereleaseConfig;
pub use publication::PublicationPolicy;
pub use signing::SignConfig;
