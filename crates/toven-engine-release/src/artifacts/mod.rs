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

pub use checksums::{ChecksumEntry, ChecksumReport, release_checksums};
pub use depgraphs::{DepgraphReport, release_depgraphs};
pub use image::{
    BuildxImagePhase, ImageModuleOutcome, ImageOptions, ImagePhaseStatus, ImageReport,
    release_image,
};
pub use package::{ArchiveFormat, PackageReport, PackagedAsset, release_package};
pub use provenance::{
    GhAttestationProvenance, ProvenanceOptions, ProvenancePhaseStatus, ProvenanceReport,
    release_provenance,
};
pub use sbom::{SbomReport, StagedSbom, release_sbom};
pub use sign::{CosignSigner, SignReport, release_sign};
pub use verify::{
    CosignVerifier, GhAssetDownloader, ProcessVersionProbe, VerifiedAsset, VerifyMode,
    VerifyOptions, VerifyReport, release_verify,
};
