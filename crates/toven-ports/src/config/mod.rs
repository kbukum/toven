//! Engine-common ecosystem configuration: the shared `[ecosystems.<id>]` knobs
//! every adapter flattens into its own strict schema.

mod common;
mod coverage;
mod hooks;
mod release;
mod run_strategy;
mod task_entry;
mod task_override;
mod units;

pub use common::CommonEcosystemConfig;
pub use coverage::{CoverageConfig, CoverageProfile, CoverageThresholds, Enforcement};
pub use hooks::HooksConfig;
pub use release::{
    BaselineSourceConfig, BumpLevel, ChangelogConfig, DelegatedTool, DependentVersion, HostConfig,
    ImageConfig, PhaseBackingKind, PhaseConfig, PhasesConfig, PrereleaseConfig, PublicationPolicy,
    ReleaseConfig, SignConfig, TagMode, VERSION_REF_TOKENS, VersionRefToken,
    VersionReferenceConfig,
};
pub use run_strategy::RunStrategy;
pub use task_entry::TaskEntry;
pub use task_override::TaskOverride;
pub use units::CompositeUnitConfig;
