//! Module-path → owning-module resolution: the shared change-ownership concern.
//!
//! The engine-owned longest-prefix change mapper attributes each changed
//! workspace-relative path to the module whose root is its longest prefix,
//! refined by adapter-declared workspace **blast-radius** globs (a `Cargo.lock`
//! change activates its whole workspace). An unclassifiable path conservatively
//! activates every module (fail-closed).
//!
//! This is a first-class shared concern, not an affected-selection internal:
//! both task-affected selection (`plan::affected`) and release change detection
//! (`toven-release`) resolve path ownership through this one module.

use std::collections::BTreeSet;
use std::path::Path;

use toven_model::{Graph, Module, ModuleKey, Workspace, WorkspaceId};
use toven_ports::ChangeRecord;

use crate::plan::discover::Federation;

/// How one changed path was attributed to workspace/module ownership.
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum Classification {
    /// Attributed to a single module by longest-prefix root match.
    Module(ModuleKey),
    /// Matched a workspace blast-radius glob (whole-workspace invalidation).
    Workspace(WorkspaceId),
    /// Could not be attributed — forces fail-closed full activation.
    Unclassified,
}

/// Resolve which module (or workspace, or none) owns a changed record.
///
/// Blast-radius globs win first (whole-workspace invalidation), then the module
/// whose root is the longest path-prefix of the record's path (or its
/// pre-rename path). A record no module root or blast-radius glob can claim is
/// [`Classification::Unclassified`], which callers fail closed on.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn owning_module(record: &ChangeRecord, federation: &Federation) -> Classification {
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

/// Map changed records to direct seed modules before any reverse-dependent
/// closure is applied.
///
/// An unclassifiable path fails closed to every module in the graph.
#[allow(clippy::redundant_pub_crate)]
pub fn changed_seeds(
    changed: &[ChangeRecord],
    graph: &Graph,
    federation: &Federation,
) -> BTreeSet<ModuleKey> {
    let mut seeds = BTreeSet::new();
    for record in changed {
        match owning_module(record, federation) {
            Classification::Module(reference) => {
                seeds.insert(reference);
            }
            Classification::Workspace(workspace) => {
                seeds.extend(modules_in_workspace(&workspace, federation));
            }
            Classification::Unclassified => return graph.modules().map(Module::key).collect(),
        }
    }
    seeds
}

/// The changed paths that no module root or workspace blast-radius glob could
/// claim.
///
/// A non-empty result is exactly the condition under which [`changed_seeds`]
/// fails closed to every module: the CLI reports these paths as the reason
/// every module was activated, so a full run is never silent. Paths are sorted
/// and de-duplicated for a stable diagnostic.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn unclassified_paths(changed: &[ChangeRecord], federation: &Federation) -> Vec<String> {
    let mut paths: Vec<String> = changed
        .iter()
        .filter(|record| {
            matches!(
                owning_module(record, federation),
                Classification::Unclassified
            )
        })
        .map(|record| record.path.display().to_string())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Return only records directly attributable to `module`.
///
/// Module-root matches belong to that one module; workspace blast-radius
/// matches belong to every module in that workspace. Unclassified records still
/// fail closed for activation through [`changed_seeds`], but they are not
/// assigned to a per-module changelog because no owner can be identified.
#[allow(clippy::redundant_pub_crate)]
pub fn changed_records_for_module(
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

fn record_belongs_to_module(
    record: &ChangeRecord,
    module: &Module,
    federation: &Federation,
) -> bool {
    match owning_module(record, federation) {
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

#[cfg(test)]
mod tests {
    use toven_model::{EcosystemId, ModuleRef, RepoPath, ToolchainTag};
    use toven_ports::ChangeStatus;

    use super::*;

    fn module(ecosystem: &str, name: &str, root: &str, workspace: Option<&str>) -> Module {
        let reference = ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap();
        let mut module = Module::new(reference, RepoPath::new(root).unwrap());
        module.workspace = workspace.map(|id| WorkspaceId::new(id).unwrap());
        module
    }

    fn rust_workspace_with_blast() -> Workspace {
        let mut workspace = Workspace::new(
            WorkspaceId::new("rust").unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("cargo"),
        );
        workspace.blast_radius = vec!["Cargo.lock".to_string()];
        workspace
    }

    fn federation() -> Federation {
        Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![
                module("rust", "app", "crates/app", Some("rust")),
                module("rust", "app-core", "crates/app-core", Some("rust")),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn key(ecosystem: &str, name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap())
    }

    #[test]
    fn owning_module_picks_longest_prefix() {
        let federation = federation();
        let record = ChangeRecord::new("crates/app-core/src/lib.rs", ChangeStatus::Modified);

        match owning_module(&record, &federation) {
            Classification::Module(reference) => assert_eq!(reference, key("rust", "app-core")),
            _ => panic!("expected longest-prefix module attribution"),
        }
    }

    #[test]
    fn owning_module_prefers_blast_radius_over_module_root() {
        let federation = federation();
        let record = ChangeRecord::new("Cargo.lock", ChangeStatus::Modified);

        match owning_module(&record, &federation) {
            Classification::Workspace(workspace) => {
                assert_eq!(workspace, WorkspaceId::new("rust").unwrap());
            }
            _ => panic!("expected blast-radius workspace attribution"),
        }
    }

    #[test]
    fn owning_module_considers_old_path_on_rename() {
        let federation = federation();
        let record = ChangeRecord::new("README.md", ChangeStatus::Renamed)
            .with_old_path("crates/app/src/lib.rs");

        match owning_module(&record, &federation) {
            Classification::Module(reference) => assert_eq!(reference, key("rust", "app")),
            _ => panic!("expected pre-rename path to attribute ownership"),
        }
    }

    #[test]
    fn owning_module_fails_closed_on_unclassified_path() {
        let federation = federation();
        let record = ChangeRecord::new("docs/guide.md", ChangeStatus::Modified);

        assert!(matches!(
            owning_module(&record, &federation),
            Classification::Unclassified
        ));
    }
}
