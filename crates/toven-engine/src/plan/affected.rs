//! Phase 5 — Affected: map changed paths to the active module set.
//!
//! The engine-owned `longest-prefix` change mapper attributes each changed
//! workspace-relative path to the module whose root is its longest prefix, refined
//! by adapter-declared workspace **blast-radius** globs (a `Cargo.lock` change
//! activates its whole workspace). It then takes the **reverse-dependents
//! closure** over the federated graph (spanning ecosystems through overlay edges)
//! and is **fail-closed**: an unclassifiable path conservatively activates every
//! module. A full run (no change filter) activates everything directly.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::AppResult;
use serde_json::Value;
use toven_model::{DepKind, Graph, Module, ModuleRef, Workspace, WorkspaceId};
use toven_ports::{ChangeRecord, TaskKind, VcsReader};

use super::discover::Federation;
use super::request::{PlanRequest, Selection};

/// Metadata key carrying a workspace's blast-radius input globs (adapter-set).
const BLAST_RADIUS_KEY: &str = "blast_radius";

/// Resolve the active module set for this request.
///
/// [`Selection::All`] activates every module; [`Selection::Changed`] maps the
/// changed paths (committed ∪ worktree) to seed modules and returns the
/// reverse-dependents closure, failing closed to the full set on any
/// unclassifiable path.
///
/// # Errors
/// Propagates [`VcsReader`] failures and the graph closure (an unknown seed).
pub(super) fn active_modules(
    request: &PlanRequest,
    graph: &Graph,
    federation: &Federation,
    vcs: &dyn VcsReader,
) -> AppResult<BTreeSet<ModuleRef>> {
    let Selection::Changed(spec) = &request.selection else {
        return Ok(all_modules(graph));
    };

    let mut changed = vcs.changed_since(spec)?;
    changed.extend(vcs.worktree_status()?);

    let seeds = changed_seeds(&changed, graph, federation);

    let is_test = matches!(request.intent, TaskKind::Test);
    let include = |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    };
    graph.closure(&seeds, include)
}

/// Map changed records to direct seed modules before any reverse-dependent
/// closure is applied.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn changed_seeds(
    changed: &[ChangeRecord],
    graph: &Graph,
    federation: &Federation,
) -> BTreeSet<ModuleRef> {
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

/// Every module identity in the graph.
fn all_modules(graph: &Graph) -> BTreeSet<ModuleRef> {
    graph.modules().map(|module| module.id.clone()).collect()
}

/// How one changed path was attributed.
enum Classification {
    /// Attributed to a single module by longest-prefix root match.
    Module(ModuleRef),
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
    let mut best: Option<(ModuleRef, usize)> = None;
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
    workspace
        .metadata
        .get(BLAST_RADIUS_KEY)
        .and_then(Value::as_array)
        .map(|globs| globs.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The module whose root is the longest path-prefix of `path` (and its depth).
fn longest_prefix(path: &Path, modules: &[Module]) -> Option<(ModuleRef, usize)> {
    let mut best: Option<(ModuleRef, usize)> = None;
    for module in modules {
        let root = module.root.as_path();
        let depth = prefix_depth(root);
        let matches = root == Path::new(".") || path.starts_with(root);
        if matches && best.as_ref().is_none_or(|(_, current)| depth > *current) {
            best = Some((module.id.clone(), depth));
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
fn modules_in_workspace(workspace: &WorkspaceId, federation: &Federation) -> Vec<ModuleRef> {
    federation
        .modules
        .iter()
        .filter(|module| module.workspace.as_ref() == Some(workspace))
        .map(|module| module.id.clone())
        .collect()
}

/// Match a repo-relative path against a `*`/`?` glob over its string rendering.
fn glob_matches(glob: &str, path: &Path) -> bool {
    wildcard(glob.as_bytes(), path.to_string_lossy().as_bytes())
}

/// Classic two-pointer wildcard matcher supporting `*` (any run) and `?` (one).
fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use toven_model::{
        AbsPath, DepKind, EcosystemId, Edge, Graph, Module, ModuleRef, RepoPath, ToolchainTag,
        Workspace, WorkspaceId,
    };
    use toven_ports::{BaselineSpec, ChangeRecord, ChangeStatus};
    use toven_testkit::FakeVcsReader;

    use super::{BLAST_RADIUS_KEY, active_modules};
    use crate::plan::discover::Federation;
    use crate::plan::request::{PlanRequest, Selection};

    fn mref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    fn module(ecosystem: &str, name: &str, root: &str, workspace: Option<&str>) -> Module {
        let mut module = Module::new(mref(ecosystem, name), RepoPath::new(root).unwrap());
        module.workspace = workspace.map(|id| WorkspaceId::new(id).unwrap());
        module
    }

    fn rust_workspace_with_blast() -> Workspace {
        let mut workspace = Workspace::new(
            WorkspaceId::new("rust").unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("cargo"),
        );
        workspace.metadata.insert(
            BLAST_RADIUS_KEY.to_string(),
            Value::Array(vec![Value::String("Cargo.lock".to_string())]),
        );
        workspace
    }

    fn request_for(changes: Vec<ChangeRecord>) -> (PlanRequest, FakeVcsReader) {
        let request = PlanRequest::new(
            "r",
            "t",
            toven_ports::TaskKind::Test,
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(BaselineSpec::explicit("main")));
        (request, FakeVcsReader::new().with_changed_since(changes))
    }

    #[test]
    fn longest_prefix_seeds_the_changed_module_and_its_dependents() {
        let federation = Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![
                module("rust", "app", "crates/app", Some("rust")),
                module("rust", "errors", "crates/errors", Some("rust")),
            ],
            edges: vec![Edge::new(
                mref("rust", "app"),
                mref("rust", "errors"),
                DepKind::Normal,
            )],
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();

        // A change under errors reaches its dependent app through the reverse closure.
        let (request, vcs) = request_for(vec![ChangeRecord::new(
            "crates/errors/lib.rs",
            ChangeStatus::Modified,
        )]);
        let active = active_modules(&request, &graph, &federation, &vcs).unwrap();
        assert!(active.contains(&mref("rust", "errors")));
        assert!(active.contains(&mref("rust", "app")));

        // A change confined to app activates only app (no dependents).
        let (request, vcs) = request_for(vec![ChangeRecord::new(
            "crates/app/lib.rs",
            ChangeStatus::Modified,
        )]);
        let active = active_modules(&request, &graph, &federation, &vcs).unwrap();
        assert_eq!(
            active,
            std::collections::BTreeSet::from([mref("rust", "app")])
        );
    }

    #[test]
    fn blast_radius_glob_activates_the_whole_workspace() {
        let federation = Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![
                module("rust", "app", "crates/app", Some("rust")),
                module("rust", "errors", "crates/errors", Some("rust")),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();

        let (request, vcs) = request_for(vec![ChangeRecord::new(
            "Cargo.lock",
            ChangeStatus::Modified,
        )]);
        let active = active_modules(&request, &graph, &federation, &vcs).unwrap();
        assert!(active.contains(&mref("rust", "app")));
        assert!(active.contains(&mref("rust", "errors")));
    }

    #[test]
    fn closure_spans_ecosystems_via_overlay() {
        let federation = Federation {
            workspaces: Vec::new(),
            modules: vec![
                module("go", "api", "services/api", Some("go")),
                module("rust", "shared", "crates/shared", Some("rust")),
            ],
            edges: vec![Edge::new(
                mref("go", "api"),
                mref("rust", "shared"),
                DepKind::Overlay,
            )],
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();

        let (request, vcs) = request_for(vec![ChangeRecord::new(
            "crates/shared/lib.rs",
            ChangeStatus::Modified,
        )]);
        let active = active_modules(&request, &graph, &federation, &vcs).unwrap();
        assert!(active.contains(&mref("rust", "shared")));
        assert!(active.contains(&mref("go", "api")));
    }

    #[test]
    fn unclassifiable_path_fails_closed_to_all_modules() {
        let federation = Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![
                module("rust", "app", "crates/app", Some("rust")),
                module("rust", "errors", "crates/errors", Some("rust")),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();

        let (request, vcs) =
            request_for(vec![ChangeRecord::new("README.md", ChangeStatus::Modified)]);
        let active = active_modules(&request, &graph, &federation, &vcs).unwrap();
        assert_eq!(active.len(), 2);
    }
}
