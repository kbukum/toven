//! Release vocabulary: the immutable planning types and resolved value types the
//! rest of the crate consumes — the release plan and its entries, bump policy
//! and overrides, resolved release/host settings, resolved targets, artifact
//! manifests, and release-tag formatting.

mod baseline_source;
mod manifest;
#[allow(clippy::module_inception)]
mod model;
mod overrides;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod settings;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod tag;
mod targets;

pub use baseline_source::BaselineSource;
pub use manifest::ArtifactManifest;
pub use model::{
    BumpPolicy, BumpReason, BumpSource, ChangelogEntry, HostRehearsal, PublishDecision, PushPolicy,
    RehearsalVerdict, ReleaseBaseline, ReleaseEntry, ReleaseModuleStatus, ReleasePlan,
    ReleaseRehearsal, ReleaseStats, ReleaseStatus,
};
pub use overrides::BumpOverrides;
pub use settings::{ResolvedHostSettings, ResolvedReleaseSettings};
#[allow(clippy::redundant_pub_crate)]
pub(crate) use targets::ReleaseTargets;
