//! Release port — the thin ecosystem sliver behind the shared release engine.

mod artifact;
mod credentials;
mod downloader;
mod host;
mod mutation;
mod outcome;
mod probe;
mod registry;
mod signer;
mod tag_scheme;
mod target;
mod verifier;

pub use artifact::Artifact;
pub use credentials::ReleaseCredentials;
pub use downloader::AssetDownloader;
pub use host::{
    HostReleaseOutcome, HostedRelease, ReleaseAsset, ReleaseHost, SUPPORTED_FORGES,
    is_supported_forge,
};
pub use mutation::ReleaseMutation;
pub use outcome::PublishOutcome;
pub use probe::VersionProbe;
pub use registry::RegistryCadence;
pub use signer::Signer;
pub use tag_scheme::TagScheme;
pub use target::ReleaseTarget;
pub use verifier::SignatureVerifier;
