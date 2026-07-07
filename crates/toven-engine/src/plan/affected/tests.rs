//! Behavioral tests for the affected-set resolver, exercised through
//! [`active_modules`](super::active_modules) and
//! [`changed_records_for_module`](super::changed_records_for_module).

use std::path::PathBuf;

use toven_model::{
    AbsPath, DepKind, EcosystemId, Edge, Graph, MemberId, Module, ModuleKey, ModuleRef, RepoPath,
    ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{BaselineSpec, ChangeRecord, ChangeStatus};
use toven_testkit::FakeVcsReader;

use super::{active_modules, changed_records_for_module};
use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};
use crate::plan::discover::Federation;
use crate::plan::request::{PlanRequest, Selection};
use toven_model::ModuleSelector;

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

    let (request, vcs) = request_for(vec![ChangeRecord::new("README.md", ChangeStatus::Modified)]);
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

fn explicit_request(
    targets: Vec<ModuleSelector>,
    include_dependents: bool,
    include_dependencies: bool,
) -> PlanRequest {
    PlanRequest::new(
        "r",
        "t",
        toven_ports::TaskKind::Test,
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Explicit {
        targets,
        include_dependents,
        include_dependencies,
    })
}

fn sel(token: &str) -> ModuleSelector {
    ModuleSelector::parse(token).unwrap()
}

fn whole_ws(token: &str) -> ModuleSelector {
    ModuleSelector::whole_workspace(token).unwrap()
}

#[test]
fn explicit_module_activates_exactly_that_module() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust:errors")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    // Without dependents, the reverse-closure dependent (app) is not activated.
    assert_eq!(
        active,
        std::collections::BTreeSet::from([mkey("rust", "errors")])
    );
}

#[test]
fn bare_name_resolves_when_unique_across_ecosystems() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("errors")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert_eq!(
        active,
        std::collections::BTreeSet::from([mkey("rust", "errors")])
    );
}

#[test]
fn bare_name_matching_two_ecosystems_is_an_ambiguity_error() {
    let (federation, graph) = ambiguous_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("core")], false, false);

    let error = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap_err();

    let message = error.to_string();
    assert!(message.contains("ambiguous"), "{message}");
    assert!(message.contains("rust:core"), "{message}");
    assert!(message.contains("go:core"), "{message}");
}

#[test]
fn bare_glob_matching_two_ecosystems_is_the_intended_set() {
    let (federation, graph) = ambiguous_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("cor*")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert!(active.contains(&mkey("rust", "core")));
    assert!(active.contains(&mkey("go", "core")));
}

#[test]
fn ecosystem_qualified_glob_scopes_to_its_ecosystem() {
    let (federation, graph) = ambiguous_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust:*")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert!(active.contains(&mkey("rust", "core")));
    assert!(!active.contains(&mkey("go", "core")));
}

#[test]
fn workspace_qualified_name_scopes_to_its_workspace() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust/app")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert_eq!(
        active,
        std::collections::BTreeSet::from([mkey("rust", "app")])
    );
}

#[test]
fn explicit_module_with_dependents_activates_the_closure() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust:errors")], true, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert!(active.contains(&mkey("rust", "errors")));
    assert!(active.contains(&mkey("rust", "app")));
}

#[test]
fn explicit_module_with_dependencies_activates_the_forward_closure() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    // app depends on errors; `--dependencies` pulls in the prerequisite.
    let request = explicit_request(vec![sel("rust:app")], false, true);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert!(active.contains(&mkey("rust", "app")));
    assert!(active.contains(&mkey("rust", "errors")));
}

#[test]
fn explicit_workspace_activates_every_owned_module() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![whole_ws("rust")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert!(active.contains(&mkey("rust", "app")));
    assert!(active.contains(&mkey("rust", "errors")));
}

#[test]
fn explicit_unknown_module_is_a_typed_error_listing_available() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust:ghost")], false, false);

    let error = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("no module matches 'rust:ghost'"),
        "{message}"
    );
    assert!(message.contains("rust:app"), "{message}");
}

#[test]
fn explicit_unknown_workspace_is_a_typed_error_listing_available() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![whole_ws("ghost")], false, false);

    let error = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("no workspace matches 'ghost'"),
        "{message}"
    );
    assert!(message.contains("rust"), "{message}");
}

#[test]
fn explicit_overlapping_targets_do_not_false_error_as_unknown() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    // Workspace `rust` already activates `rust:app`; naming it again via
    // `--module` must not be misread as an unknown module just because it
    // adds no new seed.
    let request = explicit_request(vec![whole_ws("rust"), sel("rust:app")], false, false);

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
    let request = explicit_request(vec![sel("rust:errors"), sel("rust:errors")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert_eq!(
        active,
        std::collections::BTreeSet::from([mkey("rust", "errors")])
    );
}

fn app_lib_other_federation() -> (Federation, Graph) {
    let federation = Federation {
        workspaces: vec![rust_workspace_with_blast()],
        modules: vec![
            module("rust", "app", "crates/app", Some("rust")),
            module("rust", "lib", "crates/lib", Some("rust")),
            module("rust", "base", "crates/base", Some("rust")),
            module("rust", "other", "crates/other", Some("rust")),
        ],
        edges: vec![
            Edge::new(mref("rust", "app"), mref("rust", "lib"), DepKind::Normal),
            Edge::new(mref("rust", "lib"), mref("rust", "base"), DepKind::Normal),
            Edge::new(mref("rust", "other"), mref("rust", "base"), DepKind::Normal),
        ],
        warnings: Vec::new(),
    };
    let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();
    (federation, graph)
}

#[test]
fn dependencies_and_dependents_union_excludes_prerequisite_siblings() {
    let (federation, graph) = app_lib_other_federation();
    let vcs = FakeVcsReader::new();
    // `--module lib --dependencies --dependents`: each closure runs from the
    // original seed. lib's dependency (base) and dependent (app) are added,
    // but `other` — a mere co-dependent of base — must not leak in.
    let request = explicit_request(vec![sel("rust:lib")], true, true);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();

    assert_eq!(
        active,
        std::collections::BTreeSet::from([
            mkey("rust", "lib"),
            mkey("rust", "base"),
            mkey("rust", "app"),
        ])
    );
    assert!(!active.contains(&mkey("rust", "other")));
}

fn ambiguous_federation() -> (Federation, Graph) {
    let federation = Federation {
        workspaces: vec![rust_workspace_with_blast()],
        modules: vec![
            module("rust", "core", "crates/core", Some("rust")),
            module("go", "core", "go/core", Some("go")),
        ],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();
    (federation, graph)
}
