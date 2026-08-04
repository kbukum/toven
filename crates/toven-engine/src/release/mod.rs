//! Release-specific engine vocabulary and orchestration.

pub(crate) mod apply;
mod bump;
mod bump_verb;
mod change;
mod changelog;
mod checksums;
mod conventional;
mod delegated;
mod depgraphs;
mod host;
mod manifest;
mod model;
pub(crate) mod mutate;
mod overrides;
pub(crate) mod package;
pub(crate) mod plan;
pub(crate) mod publish;
mod readiness;
mod reconcile;
mod rehearse;
mod run;
mod sbom;
mod settings;
mod sign;
mod status;
mod strategy;
pub(crate) mod tag;
mod targets;
mod verify;

pub use apply::{ReleaseApplyOptions, release_apply};
pub use bump_verb::{BumpModuleOutcome, BumpOptions, BumpReport, release_bump};
pub use checksums::{ChecksumEntry, ChecksumReport, release_checksums};
pub use delegated::{ProcessDelegatedPhase, delegated_request};
pub use depgraphs::{DepgraphReport, release_depgraphs};
pub use host::{GithubReleaseHost, GitlabReleaseHost};
pub use manifest::ArtifactManifest;
pub use model::{
    BumpPolicy, BumpReason, BumpSource, ChangelogEntry, HostRehearsal, PublishDecision, PushPolicy,
    RehearsalVerdict, ReleaseBaseline, ReleaseEntry, ReleaseModuleStatus, ReleasePlan,
    ReleaseRehearsal, ReleaseStats, ReleaseStatus,
};
pub use overrides::BumpOverrides;
pub use package::{ArchiveFormat, PackageReport, PackagedAsset, release_package};
pub use plan::release_plan;
pub use readiness::{ReadinessCheck, ReadinessReport, release_readiness};
pub use rehearse::release_rehearse;
pub use run::release_run;
pub use sbom::{SbomReport, StagedSbom, release_sbom};
pub use settings::{ResolvedHostSettings, ResolvedReleaseSettings};
pub use sign::{CosignSigner, SignReport, release_sign};
pub use status::release_status;
pub(crate) use targets::ReleaseTargets;
pub use verify::{
    CosignVerifier, GhAssetDownloader, ProcessVersionProbe, VerifiedAsset, VerifyMode,
    VerifyOptions, VerifyReport, release_verify,
};
