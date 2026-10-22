//! [`ManifestMutator`] — the version-mutation phase contract (`bump`).

use rskit_errors::AppResult;
use toven_model::{Module, RepoPath};

use super::ReleaseMutation;

/// Apply a module's version mutation to its manifest.
///
/// The manifest-write sliver of the `bump` phase: the engine owns the bump plan,
/// commit, and tag; this port only writes the version into the ecosystem's
/// manifest and reports the paths it rewrote. Object-safe so the engine can hold
/// it behind [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait ManifestMutator {
    /// Apply one atomic version mutation to the module's manifest and return the
    /// repo-relative paths it rewrote.
    ///
    /// The engine stages exactly these paths into the release commit, so the
    /// return set must name every manifest the mutation wrote. An ecosystem that
    /// carries no manifest version — a Go tag-only cut whose `go.mod` needs no
    /// dependency-floor rewrite — writes nothing and returns an empty set; the
    /// engine then tags the existing `HEAD` instead of fabricating an empty
    /// release commit.
    fn apply_release(
        &self,
        module: &Module,
        mutation: &ReleaseMutation,
    ) -> AppResult<Vec<RepoPath>>;

    /// Finish a release's manifest mutations once every module of this target
    /// has been written, and return the repo-relative paths it rewrote.
    ///
    /// `modules` are the modules whose [`apply_release`](Self::apply_release)
    /// rewrote at least one path, in plan order. An ecosystem whose package
    /// manager records workspace versions outside the manifests (an npm or
    /// pnpm lockfile) refreshes that shared file here once, rather than once
    /// per module. The engine stages these paths into the release commit with
    /// the per-module manifests. The default writes nothing.
    ///
    /// # Errors
    /// A typed error when the shared refresh fails; the release stops before
    /// the commit.
    fn finalize_release(&self, modules: &[&Module]) -> AppResult<Vec<RepoPath>> {
        let _ = modules;
        Ok(Vec::new())
    }
}
