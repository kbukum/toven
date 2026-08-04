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
}
