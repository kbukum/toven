//! Release-specific engine vocabulary and orchestration.

pub(crate) mod apply;
mod bump;
mod change;
mod changelog;
mod depgraphs;
mod host;
mod manifest;
mod model;
mod overrides;
pub(crate) mod plan;
pub(crate) mod publish;
mod readiness;
mod rehearse;
mod run;
mod sbom;
mod settings;
mod status;
mod strategy;
pub(crate) mod tag;
mod targets;

pub use apply::{ReleaseApplyOptions, release_apply};
pub use depgraphs::{DepgraphReport, release_depgraphs};
pub use host::GithubReleaseHost;
pub use manifest::ArtifactManifest;
pub use model::{
    BumpPolicy, BumpReason, BumpSource, ChangelogEntry, HostRehearsal, PublishDecision, PushPolicy,
    RehearsalVerdict, ReleaseBaseline, ReleaseEntry, ReleaseModuleStatus, ReleasePlan,
    ReleaseRehearsal, ReleaseStats, ReleaseStatus,
};
pub use overrides::BumpOverrides;
pub use plan::release_plan;
pub use readiness::{ReadinessCheck, ReadinessReport, release_readiness};
pub use rehearse::release_rehearse;
pub use run::release_run;
pub use sbom::{SbomReport, release_sbom};
pub use settings::{ResolvedHostSettings, ResolvedReleaseSettings};
pub use status::release_status;
pub(crate) use targets::ReleaseTargets;
