//! Engine-common ecosystem configuration: the shared `[ecosystems.<id>]` knobs
//! every adapter flattens into its own strict schema.

mod common;
mod coverage;
mod hooks;
mod release;
mod run_strategy;
mod task_entry;
mod task_override;

pub use common::CommonEcosystemConfig;
pub use coverage::{CoverageConfig, CoverageProfile, CoverageThresholds, Enforcement};
pub use hooks::HooksConfig;
pub use release::{
    BumpLevel, ChangelogConfig, DependentVersion, HostConfig, PrereleaseConfig, PublicationPolicy,
    ReleaseConfig, SignConfig,
};
pub use run_strategy::RunStrategy;
pub use task_entry::TaskEntry;
pub use task_override::TaskOverride;
