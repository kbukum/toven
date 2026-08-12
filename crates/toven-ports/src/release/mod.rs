//! Release port — the thin ecosystem sliver behind the shared release engine.

mod adapter;
mod artifact;
mod credentials;
mod defaults;
mod downloader;
mod host;
mod image;
mod mutation;
mod mutator;
mod outcome;
mod packager;
mod probe;
mod provenance;
mod publisher;
mod registry;
mod sbom;
mod signer;
mod tag_grammar;
mod tag_scheme;
mod verifier;
mod version;
mod visibility;

pub use adapter::ReleaseAdapter;
pub use artifact::Artifact;
pub use credentials::ReleaseCredentials;
pub use defaults::{ReleaseDefaults, ReleaseDefaultsSource};
pub use downloader::AssetDownloader;
pub use host::{
    HostReleaseOutcome, HostedRelease, ReleaseAsset, ReleaseHost, SUPPORTED_FORGES,
    is_supported_forge,
};
pub use image::{ImageOutcome, ImagePhase, ImagePublishOutcome, ImageRequest};
pub use mutation::ReleaseMutation;
pub use mutator::ManifestMutator;
pub use outcome::PublishOutcome;
pub use packager::Packager;
pub use probe::VersionProbe;
pub use provenance::{ProvenanceArtifact, ProvenanceOutcome, ProvenancePhase, ProvenanceSubject};
pub use publisher::Publisher;
pub use registry::RegistryCadence;
pub use sbom::SbomProducer;
pub use signer::Signer;
pub use tag_grammar::TagGrammar;
pub use tag_scheme::TagScheme;
pub use verifier::SignatureVerifier;
pub use version::VersionSource;
pub use visibility::Visibility;
