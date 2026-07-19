//! Changed-path classification: map changed records to seed modules.
//!
//! The engine-owned longest-prefix change mapper attributes each changed
//! workspace-relative path to the module whose root is its longest prefix, refined
//! by adapter-declared workspace **blast-radius** globs (a `Cargo.lock` change
//! activates its whole workspace). An unclassifiable path conservatively activates
//! every module (fail-closed).

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::AppResult;
use toven_model::{Graph, Module, ModuleKey, Workspace, WorkspaceId};
use toven_ports::{BaselineSpec, ChangeRecord};

use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};

use crate::plan::discover::Federation;

use super::entry::all_modules;

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn changed_for_members(
    readers: &MemberVcsReaders<'_>,
    fallback: Option<&BaselineSpec>,
) -> AppResult<Vec<ChangeRecord>> {
    let mut changed = Vec::new();
    for reader in readers.entries() {
        changed.extend(changed_for_member(reader, fallback)?);
    }
    Ok(changed)
}

/// Map one member's changed paths since its baseline.
///
/// The member reader's own resolved baseline takes precedence; when it has none
/// the request's [`Selection::Changed`](crate::plan::request::Selection::Changed)
/// spec is the fallback, so the variant's payload stays meaningful and the
/// single-repo / unconfigured-member case still resolves a baseline instead of
/// failing.
fn changed_for_member(
    reader: &MemberVcsReader<'_>,
    fallback: Option<&BaselineSpec>,
) -> AppResult<Vec<ChangeRecord>> {
    let baseline = reader.baseline().or(fallback).ok_or_else(|| {
        rskit_errors::AppError::invalid_input(
            "base_ref",
            format!(
                "no baseline reference for member '{}': pass --base <ref> or set [[members]].base_ref / [project].base_ref",
                reader.member().map_or("<root>", toven_model::MemberId::as_str)
            ),
        )
    })?;
    let mut changed = reader.reader().changed_since(baseline)?;
    changed.extend(reader.reader().worktree_status()?);
    Ok(reader.umbrella_records(&changed))
}

/// Map changed records to direct seed modules before any reverse-dependent
/// closure is applied.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn changed_seeds(
    changed: &[ChangeRecord],
    graph: &Graph,
    federation: &Federation,
) -> BTreeSet<ModuleKey> {
    let mut seeds = BTreeSet::new();
    for record in changed {
        match classify(record, federation) {
            Classification::Module(reference) => {
                seeds.insert(reference);
            }
            Classification::Workspace(workspace) => {
                seeds.extend(modules_in_workspace(&workspace, federation));
            }
            Classification::Unclassified => return all_modules(graph),
        }
    }
    seeds
}

/// The changed paths that no module root or workspace blast-radius glob could
/// claim.
///
/// A non-empty result is exactly the condition under which [`changed_seeds`]
/// fails closed to [`all_modules`]: the CLI reports these paths as the reason
/// every module was activated, so a full run is never silent. Paths are sorted
/// and de-duplicated for a stable diagnostic.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn unclassified_paths(changed: &[ChangeRecord], federation: &Federation) -> Vec<String> {
    let mut paths: Vec<String> = changed
        .iter()
        .filter(|record| matches!(classify(record, federation), Classification::Unclassified))
        .map(|record| record.path.display().to_string())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Return only records directly attributable to `module`.
///
/// Module-root matches belong to that one module; workspace blast-radius matches
/// belong to every module in that workspace. Unclassified records still fail
/// closed for activation through [`changed_seeds`], but they are not assigned to
/// a per-module changelog because no owner can be identified.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn changed_records_for_module(
    module: &Module,
    changed: &[ChangeRecord],
    federation: &Federation,
) -> Vec<ChangeRecord> {
    changed
        .iter()
        .filter(|record| record_belongs_to_module(record, module, federation))
        .cloned()
        .collect()
}

/// How one changed path was attributed.
enum Classification {
    /// Attributed to a single module by longest-prefix root match.
    Module(ModuleKey),
    /// Matched a workspace blast-radius glob (whole-workspace invalidation).
    Workspace(WorkspaceId),
    /// Could not be attributed — forces fail-closed full activation.
    Unclassified,
}

/// Classify one changed record against blast-radius globs then module roots.
fn classify(record: &ChangeRecord, federation: &Federation) -> Classification {
    for path in record_paths(record) {
        if let Some(workspace) = blast_match(path, &federation.workspaces) {
            return Classification::Workspace(workspace);
        }
    }
    let mut best: Option<(ModuleKey, usize)> = None;
    for path in record_paths(record) {
        if let Some((reference, depth)) = longest_prefix(path, &federation.modules)
            && best.as_ref().is_none_or(|(_, current)| depth > *current)
        {
            best = Some((reference, depth));
        }
    }
    best.map_or(Classification::Unclassified, |(reference, _)| {
        Classification::Module(reference)
    })
}

fn record_belongs_to_module(
    record: &ChangeRecord,
    module: &Module,
    federation: &Federation,
) -> bool {
    match classify(record, federation) {
        Classification::Module(reference) => reference == module.key(),
        Classification::Workspace(workspace) => module.workspace.as_ref() == Some(&workspace),
        Classification::Unclassified => false,
    }
}

/// The new path plus any pre-rename/-delete path of a change record.
fn record_paths(record: &ChangeRecord) -> Vec<&Path> {
    let mut paths = vec![record.path.as_path()];
    if let Some(old) = &record.old_path {
        paths.push(old.as_path());
    }
    paths
}

/// The first workspace whose blast-radius globs match `path`, if any.
fn blast_match(path: &Path, workspaces: &[Workspace]) -> Option<WorkspaceId> {
    for workspace in workspaces {
        for glob in blast_globs(workspace) {
            if glob_matches(glob, path) {
                return Some(workspace.id.clone());
            }
        }
    }
    None
}

/// The blast-radius glob strings declared on a workspace.
fn blast_globs(workspace: &Workspace) -> Vec<&str> {
    workspace.blast_radius.iter().map(String::as_str).collect()
}

/// The module whose root is the longest path-prefix of `path` (and its depth).
fn longest_prefix(path: &Path, modules: &[Module]) -> Option<(ModuleKey, usize)> {
    let mut best: Option<(ModuleKey, usize)> = None;
    for module in modules {
        let root = module.root.as_path();
        let depth = prefix_depth(root);
        let matches = root == Path::new(".") || path.starts_with(root);
        if matches && best.as_ref().is_none_or(|(_, current)| depth > *current) {
            best = Some((module.key(), depth));
        }
    }
    best
}

/// Number of path components in a module root (`.` is depth 0).
fn prefix_depth(root: &Path) -> usize {
    if root == Path::new(".") {
        0
    } else {
        root.components().count()
    }
}

/// Modules owned by a workspace.
fn modules_in_workspace(workspace: &WorkspaceId, federation: &Federation) -> Vec<ModuleKey> {
    federation
        .modules
        .iter()
        .filter(|module| module.workspace.as_ref() == Some(workspace))
        .map(Module::key)
        .collect()
}

/// Match a repo-relative path against a `*`/`?` glob over its string rendering.
fn glob_matches(glob: &str, path: &Path) -> bool {
    rskit_util::glob::glob_match(glob, &path.to_string_lossy())
}
