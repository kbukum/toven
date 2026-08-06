//! Hosted-release delivery: the per-forge `ReleaseHost` adapters and their
//! phase, the published-but-unhosted reconcile flow, the bounded publish loop,
//! and the delegated-phase process runner.

mod delegated;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod host;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod publish;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod reconcile;

pub use delegated::{ProcessDelegatedPhase, delegated_request, run_delegated_preview};
pub use host::{GithubReleaseHost, GitlabReleaseHost};
