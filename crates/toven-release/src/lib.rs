//! `toven-release` — the release PLAN/APPLY tail.
//!
//! Layer 2b of the hexagonal architecture, a peer of [`toven-engine`] sitting
//! on the shared PLAN foundation ([`toven_core`]): it owns the
//! release-specific vocabulary and orchestration — immutable release planning
//! types, change detection, bumping, changelog and manifest generation,
//! packaging, checksums, SBOM, signing, hosted-release publishing, and the
//! federated (umbrella) release bridge. It drives the release ports defined in
//! [`toven_ports`] over the config `Document`, VCS seam, PLAN spine, and
//! federation that [`toven_core`] provides. Layering stays
//! downward-only: this crate depends on [`toven_core`] and never on
//! [`toven-engine`] or [`toven_cli`](../toven_cli/index.html).
//!
//! [`toven-engine`]: ../toven_engine/index.html
#![warn(missing_docs)]

mod artifacts;
mod execution;
mod hosting;
mod model;
mod planning;
mod stream;
mod versioning;

pub use artifacts::{
    ArchiveFormat, BuildxImagePhase, ChecksumEntry, ChecksumInputs, ChecksumOperation,
    ChecksumOutcome, ChecksumReport, CosignSigner, CosignVerifier, DepgraphInputs,
    DepgraphOperation, DepgraphOutcome, DepgraphReport, GhAssetDownloader, GhAttestationProvenance,
    ImageInputs, ImageModuleOutcome, ImageOperation, ImageOptions, ImagePhaseStatus, ImageReport,
    PackageInputs, PackageOperation, PackageOutcome, PackageReport, PackagedAsset,
    ProcessVersionProbe, ProvenanceInputs, ProvenanceOperation, ProvenanceOptions,
    ProvenanceOutcome, ProvenancePhaseStatus, ProvenanceReport, ProvenanceSubjectReport,
    SbomInputs, SbomOperation, SbomOutcome, SbomReport, SignInputs, SignOperation, SignOutcome,
    SignReport, StagedSbom, VerifiedAsset, VerifyInputs, VerifyMode, VerifyOperation,
    VerifyOptions, VerifyOutcome, VerifyReport, checksums_operation, depgraph_operation,
    image_operation, package_operation, provenance_operation, release_checksums, release_depgraphs,
    release_image, release_package, release_provenance, release_sbom, release_sign, release_verify,
    sbom_operation, sign_operation, verify_operation,
};
pub use execution::{ReleaseApplyOptions, release_apply};
pub use hosting::{
    DelegatedPhaseMode, GithubReleaseHost, GitlabReleaseHost, delegated_request,
    run_delegated_preview,
};
pub(crate) use model::ReleaseTargets;
pub use model::{
    ArtifactManifest, HostRehearsal, PublishDecision, PushPolicy, RehearsalVerdict, ReleaseEntry,
    ReleaseModuleStatus, ReleasePlan, ReleaseRehearsal, ReleaseStats, ReleaseStatus,
    ResolvedHostSettings, ResolvedReleaseSettings,
};
pub use planning::{
    ReadinessCheck, ReadinessInputs, ReadinessOperation, ReadinessReport, StatusInputs,
    StatusOperation, readiness_operation, release_plan, release_readiness, release_rehearse,
    release_run, release_status, status_operation,
};
pub use toven_version::{
    BaselineSource, BumpOverrides, BumpPolicy, BumpReason, BumpSource, ChangelogEntry,
    ReleaseBaseline,
};
pub use versioning::{BumpModuleOutcome, BumpOptions, BumpReport, release_bump};
