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

use crate::versioning::changelog;
use crate::{ReleasePlan, ReleaseStats};

/// Upper bound on a changelog read; a document larger than this is treated as
/// malformed rather than loaded unbounded.
const MAX_CHANGELOG_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for the atomic changelog rewrite.
const CHANGELOG_TEMP_PREFIX: &str = "toven-changelog";

/// Apply every planned entry's manifest mutation, returning the repo-relative
/// paths each entry rewrote in plan order.
///
/// This is the manifest-write sliver both the APPLY transaction and the `bump`
/// verb run before their divergent tails. A mutation-free entry (a Go tag-only
/// cut) contributes an empty path list. `stats.mutated_modules` is incremented
/// once per entry.
///
/// # Errors
/// Propagates a missing module/target or a manifest-mutation failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn mutate_manifests(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    stats: &mut ReleaseStats,
) -> AppResult<Vec<(ModuleKey, Vec<RepoPath>)>> {
    let mut mutated = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        let module = crate::execution::apply::module_for(module_by_ref, &entry.module)?;
        let target = crate::execution::apply::target_for(targets, module)?;
        let paths = target.apply_release(module, &entry.mutation)?;
        stats.mutated_modules += 1;
        mutated.push((entry.module.clone(), paths));
    }
    Ok(mutated)
}

/// Flatten the per-entry rewritten manifest paths into the ordered set staged
/// into the release commit (or the PR-first index).
#[must_use]
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn staged_paths(mutated: &[(ModuleKey, Vec<RepoPath>)]) -> Vec<RepoPath> {
    mutated
        .iter()
        .flat_map(|(_, paths)| paths.iter().cloned())
        .collect()
}

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
