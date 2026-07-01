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
use toven_model::{DepKind, Graph, Module, ModuleKey, ModuleRef, Workspace, WorkspaceId};
use toven_ports::{BaselineSpec, ChangeRecord, TaskKind};

use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};

use super::discover::Federation;
use super::request::{ModuleSelector, PlanRequest, Selection};

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
    match &request.selection {
        Selection::All => Ok(all_modules(graph)),
        Selection::Explicit {
            targets,
            include_dependents,
        } => {
            let seeds = explicit_seeds(targets, graph, federation)?;
            if *include_dependents {
                graph.closure(&seeds, dependents_filter(&request.intent))
            } else {
                Ok(seeds)
            }
        }
        Selection::Changed(spec) => {
            let changed = changed_for_members(vcs, spec.as_ref())?;
            let seeds = changed_seeds(&changed, graph, federation);
            graph.closure(&seeds, dependents_filter(&request.intent))
        }
    }
}

/// The reverse-dependents edge filter for `intent`.
///
/// Build/normal/overlay edges always propagate; `Dev` edges propagate only for a
/// [`TaskKind::Test`] run (a dev-only change affects tests but not downstream
/// builds).
fn dependents_filter(intent: &TaskKind) -> impl Fn(DepKind) -> bool {
    let is_test = matches!(intent, TaskKind::Test);
    move |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    }
}

/// Resolve the user-named [`ModuleSelector`] targets to seed module keys.
///
/// A module target activates every graph node with that `ecosystem:name`
/// identity (one node in a single repo; every member exposing it under an
/// umbrella); a workspace target activates every module the workspace owns.
///
/// # Errors
/// A target that resolves to no discovered module is an
/// [`AppError::invalid_input`](rskit_errors::AppError::invalid_input) naming the
/// unknown ref and listing the available identities — Toven never silently plans
/// an empty run.
fn explicit_seeds(
    targets: &[ModuleSelector],
    graph: &Graph,
    federation: &Federation,
) -> AppResult<BTreeSet<ModuleKey>> {
    let mut seeds = BTreeSet::new();
    for target in targets {
        match target {
            ModuleSelector::Module(reference) => {
                let matches: Vec<ModuleKey> = graph
                    .modules()
                    .map(Module::key)
                    .filter(|key| key.module() == reference)
                    .collect();
                if matches.is_empty() {
                    return Err(unknown_module_error(reference, graph));
                }
                seeds.extend(matches);
            }
            ModuleSelector::Workspace(workspace) => {
                let matches = modules_in_workspace(workspace, federation);
                if matches.is_empty() {
                    return Err(unknown_workspace_error(workspace, federation));
                }
                seeds.extend(matches);
            }
        }
    }
    Ok(seeds)
}

/// Typed error for a `--module` ref that matches no discovered module.
fn unknown_module_error(reference: &ModuleRef, graph: &Graph) -> rskit_errors::AppError {
    let mut available: Vec<String> = graph
        .modules()
        .map(|module| module.key().module().to_string())
        .collect();
    available.sort();
    available.dedup();
    rskit_errors::AppError::invalid_input(
        "module",
        format!(
            "unknown module '{reference}'; discovered modules: {}",
            available.join(", ")
        ),
    )
}

/// Typed error for a `--workspace` id that owns no discovered module.
fn unknown_workspace_error(
    workspace: &WorkspaceId,
    federation: &Federation,
) -> rskit_errors::AppError {
    let mut available: Vec<String> = federation
        .workspaces
        .iter()
        .map(|workspace| workspace.id.to_string())
        .collect();
    available.sort();
    available.dedup();
    rskit_errors::AppError::invalid_input(
        "workspace",
        format!(
            "unknown workspace '{workspace}'; discovered workspaces: {}",
            available.join(", ")
        ),
    )
}

fn changed_for_members(
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
/// the request's [`Selection::Changed`] spec is the fallback, so the variant's
/// payload stays meaningful and the single-repo / unconfigured-member case still
/// resolves a baseline instead of failing.
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
    use crate::plan::request::{ModuleSelector, PlanRequest, Selection};

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
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
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
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));

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
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));

        let active = active_modules(&request, &graph, &federation, &readers).unwrap();

        assert!(active.contains(&ModuleKey::new(None, mref("rust", "shared"))));
    }

    #[test]
    fn member_without_a_baseline_and_no_request_fallback_is_rejected() {
        let shared = module("rust", "shared", "crates/shared", Some("rust"));
        let federation = Federation {
            workspaces: vec![rust_workspace_with_blast()],
            modules: vec![shared],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();
        let vcs = FakeVcsReader::new();
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
        .with_selection(Selection::Changed(None));

        let error = active_modules(&request, &graph, &federation, &readers).unwrap_err();

        assert!(error.to_string().contains("no baseline reference"));
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

    fn app_and_errors_federation() -> (Federation, Graph) {
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
        (federation, graph)
    }

    fn explicit_request(targets: Vec<ModuleSelector>, include_dependents: bool) -> PlanRequest {
        PlanRequest::new(
            "r",
            "t",
            toven_ports::TaskKind::Test,
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Explicit {
            targets,
            include_dependents,
        })
    }

    #[test]
    fn explicit_module_activates_exactly_that_module() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        let request = explicit_request(vec![ModuleSelector::Module(mref("rust", "errors"))], false);

        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

        // Without dependents, the reverse-closure dependent (app) is not activated.
        assert_eq!(
            active,
            std::collections::BTreeSet::from([mkey("rust", "errors")])
        );
    }

    #[test]
    fn explicit_module_with_dependents_activates_the_closure() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        let request = explicit_request(vec![ModuleSelector::Module(mref("rust", "errors"))], true);

        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

        assert!(active.contains(&mkey("rust", "errors")));
        assert!(active.contains(&mkey("rust", "app")));
    }

    #[test]
    fn explicit_workspace_activates_every_owned_module() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        let request = explicit_request(
            vec![ModuleSelector::Workspace(WorkspaceId::new("rust").unwrap())],
            false,
        );

        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

        assert!(active.contains(&mkey("rust", "app")));
        assert!(active.contains(&mkey("rust", "errors")));
    }

    #[test]
    fn explicit_unknown_module_is_a_typed_error_listing_available() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        let request = explicit_request(vec![ModuleSelector::Module(mref("rust", "ghost"))], false);

        let error = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("unknown module 'rust:ghost'"), "{message}");
        assert!(message.contains("rust:app"), "{message}");
    }

    #[test]
    fn explicit_unknown_workspace_is_a_typed_error_listing_available() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        let request = explicit_request(
            vec![ModuleSelector::Workspace(
                WorkspaceId::new("ghost").unwrap(),
            )],
            false,
        );

        let error = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("unknown workspace 'ghost'"), "{message}");
        assert!(message.contains("rust"), "{message}");
    }

    #[test]
    fn explicit_overlapping_targets_do_not_false_error_as_unknown() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        // Workspace `rust` already activates `rust:app`; naming it again via
        // `--module` must not be misread as an unknown module just because it
        // adds no new seed.
        let request = explicit_request(
            vec![
                ModuleSelector::Workspace(WorkspaceId::new("rust").unwrap()),
                ModuleSelector::Module(mref("rust", "app")),
            ],
            false,
        );

        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

        assert!(active.contains(&mkey("rust", "app")));
        assert!(active.contains(&mkey("rust", "errors")));
    }

    #[test]
    fn explicit_duplicate_module_targets_are_idempotent() {
        let (federation, graph) = app_and_errors_federation();
        let vcs = FakeVcsReader::new();
        // A repeated `--module` for the same identity must succeed, not error on
        // the second (already-present) occurrence.
        let request = explicit_request(
            vec![
                ModuleSelector::Module(mref("rust", "errors")),
                ModuleSelector::Module(mref("rust", "errors")),
            ],
            false,
        );

        let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

        assert_eq!(
            active,
            std::collections::BTreeSet::from([mkey("rust", "errors")])
        );
    }
}
