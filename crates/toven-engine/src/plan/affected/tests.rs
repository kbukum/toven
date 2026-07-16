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
        toven_ports::TaskIntent::resolve("test"),
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
    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;
    assert!(active.contains(&mkey("rust", "errors")));
    assert!(active.contains(&mkey("rust", "app")));

    // A change confined to app activates only app (no dependents).
    let (request, vcs) = request_for(vec![ChangeRecord::new(
        "crates/app/lib.rs",
        ChangeStatus::Modified,
    )]);
    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;
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
    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;
    assert!(active.contains(&mkey("rust", "app")));
    assert!(active.contains(&mkey("rust", "errors")));
}

#[test]
fn dirty_worktree_changes_seed_the_affected_set() {
    // Uncommitted working-tree edits are part of the affected input, unioned
    // with the committed diff, so a dirty checkout plans the same modules a
    // commit would — here an uncommitted edit under errors reaches its dependent
    // app through the reverse closure even with nothing committed.
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

    let request = PlanRequest::new(
        "r",
        "t",
        toven_ports::TaskIntent::resolve("test"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_changed_since(Vec::new())
        .with_worktree_status(vec![ChangeRecord::new(
            "crates/errors/lib.rs",
            ChangeStatus::Modified,
        )]);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;
    assert!(
        active.contains(&mkey("rust", "errors")),
        "the uncommitted change seeds its owning module"
    );
    assert!(
        active.contains(&mkey("rust", "app")),
        "the reverse closure reaches the dependent"
    );
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
    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;
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
    assert_eq!(active.modules.len(), 2);
    assert_eq!(
        active.full_activation,
        vec!["README.md".to_string()],
        "an unattributable root path must be reported as the full-activation reason"
    );
}

#[test]
fn an_attributable_change_reports_no_full_activation() {
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
        "crates/app/lib.rs",
        ChangeStatus::Modified,
    )]);
    let active = active_modules(&request, &graph, &federation, &single_view(&vcs)).unwrap();
    assert_eq!(
        active.modules,
        std::collections::BTreeSet::from([mkey("rust", "app")])
    );
    assert!(
        active.full_activation.is_empty(),
        "a precisely attributed change must not force full activation"
    );
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
        toven_ports::TaskIntent::resolve("test"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));

    let active = active_modules(&request, &graph, &federation, &readers)
        .unwrap()
        .modules;

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
        toven_ports::TaskIntent::resolve("test"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));

    let active = active_modules(&request, &graph, &federation, &readers)
        .unwrap()
        .modules;

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
        toven_ports::TaskIntent::resolve("test"),
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
    explicit_request_with_intent(
        targets,
        include_dependents,
        include_dependencies,
        toven_ports::TaskIntent::resolve("test"),
    )
}

fn explicit_request_with_intent(
    targets: Vec<ModuleSelector>,
    include_dependents: bool,
    include_dependencies: bool,
    intent: toven_ports::TaskIntent,
) -> PlanRequest {
    PlanRequest::new("r", "t", intent, AbsPath::new("/repo").unwrap()).with_selection(
        Selection::Explicit {
            targets,
            include_dependents,
            include_dependencies,
        },
    )
}

fn app_dev_dep_errors_federation() -> (Federation, Graph) {
    let federation = Federation {
        workspaces: vec![rust_workspace_with_blast()],
        modules: vec![
            module("rust", "app", "crates/app", Some("rust")),
            module("rust", "errors", "crates/errors", Some("rust")),
        ],
        edges: vec![Edge::new(
            mref("rust", "app"),
            mref("rust", "errors"),
            DepKind::Dev,
        )],
        warnings: Vec::new(),
    };
    let graph = Graph::build(federation.modules.clone(), federation.edges.clone()).unwrap();
    (federation, graph)
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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

    assert!(active.contains(&mkey("rust", "core")));
    assert!(active.contains(&mkey("go", "core")));
}

#[test]
fn ecosystem_qualified_glob_scopes_to_its_ecosystem() {
    let (federation, graph) = ambiguous_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust:*")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

    assert!(active.contains(&mkey("rust", "core")));
    assert!(!active.contains(&mkey("go", "core")));
}

#[test]
fn workspace_qualified_name_scopes_to_its_workspace() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![sel("rust/app")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

    assert!(active.contains(&mkey("rust", "errors")));
    assert!(active.contains(&mkey("rust", "app")));
}

#[test]
fn explicit_module_with_dependencies_activates_the_forward_closure() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    // app depends on errors; `--dependencies` pulls in the prerequisite.
    let request = explicit_request(vec![sel("rust:app")], false, true);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

    assert!(active.contains(&mkey("rust", "app")));
    assert!(active.contains(&mkey("rust", "errors")));
}

#[test]
fn dev_only_dependency_is_excluded_from_the_forward_closure_of_a_build() {
    let (federation, graph) = app_dev_dep_errors_federation();
    let vcs = FakeVcsReader::new();
    // app dev-depends on errors; `--dependencies` on a build must not pull it in.
    let request = explicit_request_with_intent(
        vec![sel("rust:app")],
        false,
        true,
        toven_ports::TaskIntent::resolve("build"),
    );

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

    assert_eq!(
        active,
        std::collections::BTreeSet::from([mkey("rust", "app")])
    );
}

#[test]
fn dev_only_dependency_is_included_in_the_forward_closure_of_a_test() {
    let (federation, graph) = app_dev_dep_errors_federation();
    let vcs = FakeVcsReader::new();
    // app dev-depends on errors; `--dependencies` on a test needs the prerequisite.
    let request = explicit_request_with_intent(
        vec![sel("rust:app")],
        false,
        true,
        toven_ports::TaskIntent::resolve("test"),
    );

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

    assert!(active.contains(&mkey("rust", "app")));
    assert!(active.contains(&mkey("rust", "errors")));
}

#[test]
fn explicit_workspace_activates_every_owned_module() {
    let (federation, graph) = app_and_errors_federation();
    let vcs = FakeVcsReader::new();
    let request = explicit_request(vec![whole_ws("rust")], false, false);

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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

    let active = active_modules(&request, &graph, &federation, &single_view(&vcs))
        .unwrap()
        .modules;

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
