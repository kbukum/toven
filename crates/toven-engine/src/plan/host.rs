//! The injected host effects the PLAN pipeline depends on.
//!
//! The pipeline itself is pure orchestration; every
//! filesystem/git/process/cache side effect is reached through this bundle of
//! injected ports, so tests drive a fully deterministic plan with fakes.

use crate::federation::baseline::MemberVcsReaders;
use toven_ports::{CacheStore, SourceDigest, ToolchainProber};

/// The injected side-effect ports a [`plan`](super::plan) run uses.
///
/// Grouped so the pure phase functions stay individually testable while the
/// public entry takes one cohesive host handle. The git seam is always the
/// member-indexed [`MemberVcsReaders`] view: a single-repo project is the N=1
/// degenerate member (no id, empty prefix) rather than a separate code path.
#[derive(Clone, Copy)]
pub struct PlanHost<'a> {
    /// Read-only git seams for changed-path detection, one per member repo.
    pub vcs: &'a MemberVcsReaders<'a>,
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
        vcs: &'a MemberVcsReaders<'a>,
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
