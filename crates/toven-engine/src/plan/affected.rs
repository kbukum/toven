//! Affected: map changed paths to the active module set.
//!
//! The engine-owned `longest-prefix` change mapper attributes each changed
//! workspace-relative path to the module whose root is its longest prefix, refined
//! by adapter-declared workspace **blast-radius** globs (a `Cargo.lock` change
//! activates its whole workspace). It then takes the **reverse-dependents
//! closure** over the federated graph (spanning ecosystems through overlay edges)
//! and is **fail-closed**: an unclassifiable path conservatively activates every
//! module. A full run (no change filter) activates everything directly.

use rskit_errors::AppResult;
use std::collections::BTreeSet;
use std::path::Path;
use toven_model::{DepKind, Graph, Module, ModuleKey, Workspace, WorkspaceId};
use toven_ports::{BaselineSpec, ChangeRecord, TaskKind};

use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders, prefix_records};

use super::discover::Federation;
use super::request::{PlanRequest, Selection};

/// Resolve the active module set for this request.
///
/// [`Selection::All`] activates every module; [`Selection::Changed`] maps the
/// changed paths (committed ∪ worktree) reported by every member reader to seed
/// modules and returns the reverse-dependents closure, failing closed to the full
/// set on any unclassifiable path. Each member reader uses its own resolved
/// baseline; the single-repo project is the N=1 degenerate member.
///
/// # Errors
/// Propagates [`VcsReader`](toven_ports::VcsReader) failures and the graph
/// closure (an unknown seed).
pub(super) fn active_modules(
    request: &PlanRequest,
    graph: &Graph,
    federation: &Federation,
    vcs: &MemberVcsReaders<'_>,
) -> AppResult<BTreeSet<ModuleKey>> {
    let Selection::Changed(spec) = &request.selection else {
        return Ok(all_modules(graph));
    };

    let changed = changed_for_members(vcs, spec)?;

    let seeds = changed_seeds(&changed, graph, federation);

    let is_test = matches!(request.intent, TaskKind::Test);
    let include = |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    };
    graph.closure(&seeds, include)
}

fn changed_for_members(
    readers: &MemberVcsReaders<'_>,
    fallback: &BaselineSpec,
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
/// the request's [`Selection::Changed`] spec is the fallback, so the variant's
/// payload stays meaningful and the single-repo / unconfigured-member case still
/// resolves a baseline instead of failing.
fn changed_for_member(
    reader: &MemberVcsReader<'_>,
    fallback: &BaselineSpec,
) -> AppResult<Vec<ChangeRecord>> {
    let baseline = reader.baseline().unwrap_or(fallback);
    let mut changed = reader.reader().changed_since(baseline)?;
    changed.extend(reader.reader().worktree_status()?);
    Ok(prefix_records(&changed, reader.prefix()))
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

/// Every module key in the graph.
fn all_modules(graph: &Graph) -> BTreeSet<ModuleKey> {
    graph.modules().map(Module::key).collect()
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
    use std::path::PathBuf;

    use toven_model::{
        AbsPath, DepKind, EcosystemId, Edge, Graph, MemberId, Module, ModuleKey, ModuleRef,
        RepoPath, ToolchainTag, Workspace, WorkspaceId,
    };
    use toven_ports::{BaselineSpec, ChangeRecord, ChangeStatus};
    use toven_testkit::FakeVcsReader;

    use super::{active_modules, changed_records_for_module};
    use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};
    use crate::plan::discover::Federation;
    use crate::plan::request::{PlanRequest, Selection};

    fn mref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    fn mkey(ecosystem: &str, name: &str) -> ModuleKey {
        ModuleKey::bare(mref(ecosystem, name))
    }

    fn member_key(member: &str, ecosystem: &str, name: &str) -> ModuleKey {
        ModuleKey::new(Some(MemberId::new(member).unwrap()), mref(ecosystem, name))
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
        workspace.blast_radius = vec!["Cargo.lock".to_string()];
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

    fn single_view(vcs: &FakeVcsReader) -> MemberVcsReaders<'_> {
        MemberVcsReaders::single(vcs, BaselineSpec::explicit("main"))
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
        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();
        assert!(active.contains(&mkey("rust", "errors")));
        assert!(active.contains(&mkey("rust", "app")));

        // A change confined to app activates only app (no dependents).
        let (request, vcs) = request_for(vec![ChangeRecord::new(
            "crates/app/lib.rs",
            ChangeStatus::Modified,
        )]);
        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();
        assert_eq!(
            active,
            std::collections::BTreeSet::from([mkey("rust", "app")])
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
        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();
        assert!(active.contains(&mkey("rust", "app")));
        assert!(active.contains(&mkey("rust", "errors")));
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
        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();
        assert!(active.contains(&mkey("rust", "shared")));
        assert!(active.contains(&mkey("go", "api")));
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
        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn member_readers_prefix_repo_changes_before_classification() {
        let core = MemberId::new("core").unwrap();
        let gateway = MemberId::new("gateway").unwrap();
        let mut shared = module(
            "rust",
            "shared",
            "repos/core/crates/shared",
            Some("core/rust"),
        );
        shared.member = Some(core.clone());
        let mut api = module(
            "rust",
            "api",
            "repos/gateway/crates/api",
            Some("gateway/rust"),
        );
        api.member = Some(gateway.clone());
        let federation = Federation {
            workspaces: Vec::new(),
            modules: vec![shared, api],
            edges: vec![Edge::new(
                member_key("gateway", "rust", "api"),
                member_key("core", "rust", "shared"),
                DepKind::Overlay,
            )],
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();
        let core_vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/shared/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let gateway_vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::new(vec![
            MemberVcsReader::new(
                Some(core),
                PathBuf::from("repos/core"),
                Some(BaselineSpec::explicit("main")),
                &core_vcs,
            ),
            MemberVcsReader::new(
                Some(gateway),
                PathBuf::from("repos/gateway"),
                Some(BaselineSpec::explicit("main")),
                &gateway_vcs,
            ),
        ]);
        let request = PlanRequest::new(
            "r",
            "t",
            toven_ports::TaskKind::Test,
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(BaselineSpec::explicit("main")));

        let active = active_modules(&request, &graph, &federation, &readers).unwrap();

        assert!(active.contains(&member_key("core", "rust", "shared")));
        assert!(active.contains(&member_key("gateway", "rust", "api")));
    }

    #[test]
    fn member_without_a_baseline_falls_back_to_the_request_spec() {
        let shared = module("rust", "shared", "crates/shared", Some("rust"));
        let federation = Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![shared],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/shared/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        // The reader carries no baseline of its own, so change detection must use
        // the request's `Selection::Changed` spec as the fallback.
        let readers = MemberVcsReaders::new(vec![MemberVcsReader::new(
            None,
            PathBuf::from(""),
            None,
            &vcs,
        )]);
        let request = PlanRequest::new(
            "r",
            "t",
            toven_ports::TaskKind::Test,
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(BaselineSpec::explicit("main")));

        let active = active_modules(&request, &graph, &federation, &readers).unwrap();

        assert!(active.contains(&ModuleKey::new(None, mref("rust", "shared"))));
    }

    #[test]
    fn changed_records_for_module_keeps_only_owned_and_workspace_changes() {
        let app = module("rust", "app", "crates/app", Some("rust"));
        let errors = module("rust", "errors", "crates/errors", Some("rust"));
        let foreign = module("go", "api", "services/api", Some("go"));
        let federation = Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![app.clone(), errors, foreign],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let changes = vec![
            ChangeRecord::new("crates/app/src/lib.rs", ChangeStatus::Modified),
            ChangeRecord::new("crates/errors/src/lib.rs", ChangeStatus::Modified),
            ChangeRecord::new("Cargo.lock", ChangeStatus::Modified),
            ChangeRecord::new("README.md", ChangeStatus::Modified),
        ];

        let records = changed_records_for_module(&app, &changes, &federation);

        assert_eq!(
            records
                .iter()
                .map(|record| record.path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["crates/app/src/lib.rs", "Cargo.lock"]
        );
    }
}
