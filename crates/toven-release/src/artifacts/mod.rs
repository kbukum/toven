//! Release artifacts and supply chain: the declared hosted assets, packaging,
//! checksums, SBOM, dependency-graph projection, signing, and artifact
//! verification.

mod assets;
mod checksums;
mod depgraphs;
mod image;
mod package;
mod provenance;
mod sbom;
mod sign;
mod verify;

pub use checksums::{
    ChecksumEntry, ChecksumInputs, ChecksumOperation, ChecksumOutcome, ChecksumReport,
    checksums_operation, release_checksums,
};
pub use depgraphs::{
    DepgraphInputs, DepgraphOperation, DepgraphOutcome, DepgraphReport, depgraph_operation,
    release_depgraphs,
};
pub use image::{
    BuildxImagePhase, ImageInputs, ImageModuleOutcome, ImageOperation, ImageOptions,
    ImagePhaseStatus, ImageReport, image_operation, release_image,
};
pub use package::{
    ArchiveFormat, PackageInputs, PackageOperation, PackageOutcome, PackageReport, PackagedAsset,
    package_operation, release_package,
};
pub use provenance::{
    GhAttestationProvenance, ProvenanceInputs, ProvenanceOperation, ProvenanceOptions,
    ProvenanceOutcome, ProvenancePhaseStatus, ProvenanceReport, ProvenanceSubjectReport,
    provenance_operation, release_provenance,
};
pub use sbom::{
    SbomInputs, SbomOperation, SbomOutcome, SbomReport, StagedSbom, release_sbom, sbom_operation,
};
pub use sign::{
    CosignSigner, SignInputs, SignOperation, SignOutcome, SignReport, release_sign, sign_operation,
};
pub use verify::{
    CosignVerifier, GhAssetDownloader, ProcessVersionProbe, VerifiedAsset, VerifyInputs,
    VerifyMode, VerifyOperation, VerifyOptions, VerifyOutcome, VerifyReport, release_verify,
    verify_operation,
};
