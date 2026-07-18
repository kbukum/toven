//! Release port — the thin ecosystem sliver behind the shared release engine.

mod artifact;
mod host;
mod mutation;
mod outcome;
mod registry;
mod tag_scheme;
mod target;

pub use artifact::Artifact;
pub use host::{
    HostReleaseOutcome, HostedRelease, ReleaseAsset, ReleaseHost, SUPPORTED_FORGES,
    is_supported_forge,
};
pub use mutation::ReleaseMutation;
pub use outcome::PublishOutcome;
pub use registry::RegistryCadence;
pub use tag_scheme::TagScheme;
pub use target::ReleaseTarget;
