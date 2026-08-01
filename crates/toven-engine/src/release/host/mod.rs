//! The hosted-release phase and its per-forge [`ReleaseHost`] adapters.
//!
//! [`phase`] is the forge-agnostic glue (resolve planned Releases, build the
//! forge host registry, run the phase); each sibling module is one concrete
//! forge adapter, wired into `build_hosts` by its configured `forge`
//! identifier. GitLab and other forges are added here as new adapter modules
//! behind the same [`ReleaseHost`](toven_ports::ReleaseHost) port.

mod github;
mod gitlab;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod phase;

pub use github::GithubReleaseHost;
pub use gitlab::GitlabReleaseHost;
#[allow(clippy::redundant_pub_crate)]
pub(crate) use phase::{
    PlannedHostRelease, ReleaseHosts, build_hosts, planned_host_releases, run_host_phase,
};
