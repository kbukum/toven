//! The shared `bump`-phase mutation prefix.
//!
//! Both the release APPLY transaction and the standalone `release bump` verb
//! begin by writing every planned version into its manifest through the
//! [`ManifestMutator`](toven_ports::ManifestMutator) port and, where configured,
//! rolling the changelog. Factoring that prefix here keeps the two tails
//! (APPLY's commit → tag → publish versus `bump`'s commit-or-stage) from
//! duplicating the version-write and changelog-roll loops.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::{read_string_bounded, write_atomic};
use toven_model::{Module, ModuleKey, RepoPath};

use crate::{ReleasePlan, ReleaseStats};
use toven_version::changelog;

/// Per-module rewritten manifest paths in plan order: for each module the
/// repo-relative paths its manifest mutation rewrote (empty for a tag-only cut).
#[allow(clippy::redundant_pub_crate)]
pub(crate) type MutatedManifests = Vec<(ModuleKey, Vec<RepoPath>)>;

/// Upper bound on a changelog read; a document larger than this is treated as
/// malformed rather than loaded unbounded.
const MAX_CHANGELOG_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for the atomic changelog rewrite.
const CHANGELOG_TEMP_PREFIX: &str = "toven-changelog";

/// The manifest writes of one release: each entry's rewritten paths in plan
/// order, plus the shared paths its targets rewrote once for the whole release
/// (a refreshed lockfile).
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct ManifestWrites {
    /// Per-module rewritten manifest paths in plan order.
    pub(crate) per_module: MutatedManifests,
    /// Paths a target's [`finalize_release`](toven_ports::ManifestMutator::finalize_release)
    /// rewrote, in target first-seen order.
    pub(crate) shared: Vec<RepoPath>,
}

impl ManifestWrites {
    /// The ordered, de-duplicated set staged into the release commit (or the
    /// PR-first index): every per-module path, then every shared path.
    ///
    /// Duplicates are collapsed in first-seen order: a single-version
    /// workspace routes every member's version bump to one shared
    /// workspace-root manifest, so many entries report the same path. Staging
    /// it once keeps the staged set and the release commit honest.
    #[must_use]
    pub(crate) fn staged_paths(&self) -> Vec<RepoPath> {
        let mut seen = BTreeSet::new();
        self.per_module
            .iter()
            .flat_map(|(_, paths)| paths.iter())
            .chain(self.shared.iter())
            .filter(|path| seen.insert((*path).clone()))
            .cloned()
            .collect()
    }
}

/// Apply every planned entry's manifest mutation, then let each target finish
/// the release once over the modules it rewrote.
///
/// This is the manifest-write sliver both the APPLY transaction and the `bump`
/// verb run before their divergent tails. A mutation-free entry (a Go tag-only
/// cut) contributes an empty path list and is not passed to its target's
/// finalize step; a target with no rewritten module is not finalized at all.
/// `stats.mutated_modules` is incremented once per entry.
///
/// # Errors
/// Propagates a missing module/target, a manifest-mutation failure, or a
/// finalize failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn mutate_manifests(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    stats: &mut ReleaseStats,
) -> AppResult<ManifestWrites> {
    let mut per_module = Vec::with_capacity(plan.entries.len());
    let mut rewritten: Vec<(TargetKey, Vec<&Module>)> = Vec::new();
    for entry in &plan.entries {
        let module = crate::execution::apply::module_for(module_by_ref, &entry.module)?;
        let target = crate::execution::apply::target_for(targets, module)?;
        let paths = target.apply_release(module, &entry.mutation)?;
        stats.mutated_modules += 1;
        if !paths.is_empty() {
            let key = (module.member.clone(), module.id.ecosystem.clone());
            match rewritten.iter_mut().find(|(existing, _)| *existing == key) {
                Some((_, modules)) => modules.push(module),
                None => rewritten.push((key, vec![module])),
            }
        }
        per_module.push((entry.module.clone(), paths));
    }
    let mut shared = Vec::new();
    for (_, modules) in &rewritten {
        let target = crate::execution::apply::target_for(targets, modules[0])?;
        shared.extend(target.finalize_release(modules)?);
    }
    Ok(ManifestWrites { per_module, shared })
}

/// The release-target key: one target per `(member, ecosystem)`.
type TargetKey = (Option<toven_model::MemberId>, toven_model::EcosystemId);

/// Roll each opted-in changelog once, moving its documented `## [Unreleased]`
/// body under a versioned `## [version] - date` heading, and return the
/// repo-relative changelog paths that were rewritten.
///
/// Only entries whose module configured `changelog.roll` participate, and each
/// distinct changelog file is rolled once (a single-version workspace maps many
/// modules onto one changelog and one shared version). A changelog with no
/// documented `[Unreleased]` entry is left untouched and contributes no path.
/// `root` is the member repository root the changelog paths are relative to.
///
/// # Errors
/// Propagates an unsafe changelog path, an unreadable changelog, or a write
/// failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn roll_changelogs(
    plan: &ReleasePlan,
    root: &Path,
    date: &str,
) -> AppResult<Vec<RepoPath>> {
    let mut rolled_files = BTreeSet::new();
    let mut changed = Vec::new();
    for entry in &plan.entries {
        if !entry.changelog_roll {
            continue;
        }
        let Some(version) = &entry.planned_version else {
            continue;
        };
        if !rolled_files.insert(entry.changelog_path.clone()) {
            continue;
        }
        let relative = entry.changelog_path.as_str();
        let absolute = safe_join(root, relative).map_err(|error| {
            AppError::invalid_input(
                "release.changelog.path",
                format!("changelog path '{relative}' is not a safe project-relative path"),
            )
            .with_cause(error)
        })?;
        let text = read_string_bounded(&absolute, MAX_CHANGELOG_BYTES).map_err(|error| {
            AppError::invalid_input(
                "release.changelog.roll",
                format!(
                    "changelog '{relative}' could not be read to roll its '[Unreleased]' section"
                ),
            )
            .with_cause(error)
        })?;
        let Some(rolled) = changelog::roll_unreleased(&text, version, date) else {
            continue;
        };
        write_atomic(&absolute, rolled.as_bytes(), CHANGELOG_TEMP_PREFIX)?;
        changed.push(RepoPath::new(relative)?);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use toven_model::{ModuleKey, ModuleRef, RepoPath};

    use super::ManifestWrites;

    fn staged_paths(per_module: &[(ModuleKey, Vec<RepoPath>)]) -> Vec<RepoPath> {
        ManifestWrites {
            per_module: per_module.to_vec(),
            shared: Vec::new(),
        }
        .staged_paths()
    }

    fn key(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::parse(name).expect("module ref"))
    }

    #[test]
    fn staged_paths_collapses_a_shared_root_in_first_seen_order() {
        // A single-version workspace routes every member's bump to one shared
        // workspace-root manifest, so many entries report the same path. The
        // staged set must carry it once, in first-seen order.
        let root = RepoPath::new("Cargo.toml").unwrap();
        let mutated = vec![
            (key("rust:app"), vec![root.clone()]),
            (key("rust:lib"), vec![root.clone()]),
            (key("rust:core"), vec![root.clone()]),
        ];
        assert_eq!(staged_paths(&mutated), vec![root]);
    }

    #[test]
    fn staged_paths_preserves_distinct_per_member_manifests() {
        let app = RepoPath::new("crates/app/Cargo.toml").unwrap();
        let lib = RepoPath::new("crates/lib/Cargo.toml").unwrap();
        let mutated = vec![
            (key("rust:app"), vec![app.clone()]),
            (key("rust:lib"), vec![lib.clone()]),
        ];
        assert_eq!(staged_paths(&mutated), vec![app, lib]);
    }

    #[test]
    fn staged_paths_appends_shared_paths_once() {
        let app = RepoPath::new("packages/app/package.json").unwrap();
        let lock = RepoPath::new("package-lock.json").unwrap();
        let writes = ManifestWrites {
            per_module: vec![(key("js:app"), vec![app.clone()])],
            shared: vec![lock.clone(), app.clone()],
        };
        assert_eq!(writes.staged_paths(), vec![app, lock]);
    }
}
