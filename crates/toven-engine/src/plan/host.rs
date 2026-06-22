//! The injected host effects the PLAN pipeline depends on.
//!
//! The pipeline itself is pure orchestration; every filesystem/git/process/cache
//! side effect is reached through this bundle of injected ports, so tests drive a
//! fully deterministic plan with fakes.

use toven_ports::VcsReader;

use super::cache::CacheStore;
use super::source::SourceDigest;
use super::toolchain::ToolchainProber;

/// The injected side-effect ports a [`plan`](super::plan) run uses.
///
/// Grouped so the pure phase functions stay individually testable while the
/// public entry takes one cohesive host handle.
#[derive(Clone, Copy)]
pub struct PlanHost<'a> {
    /// Read-only git seam for changed-path detection.
    pub vcs: &'a dyn VcsReader,
    /// Content digest for module/source cache identities.
    pub digest: &'a dyn SourceDigest,
    /// Toolchain version prober for active workspaces.
    pub prober: &'a dyn ToolchainProber,
    /// Existing-record lookup for cache verdicts.
    pub cache: &'a dyn CacheStore,
}

impl<'a> PlanHost<'a> {
    /// Bundle the injected ports into one host handle.
    #[must_use]
    pub const fn new(
        vcs: &'a dyn VcsReader,
        digest: &'a dyn SourceDigest,
        prober: &'a dyn ToolchainProber,
        cache: &'a dyn CacheStore,
    ) -> Self {
        Self {
            vcs,
            digest,
            prober,
            cache,
        }
    }
}
