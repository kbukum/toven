//! End-to-end cross-repo umbrella PLAN tests.
//!
//! Each test builds a real multi-member umbrella on disk (an umbrella
//! `toven.toml` plus every member's own `toven.toml`) and drives it through the
//! public discovery/plan front. The assertions prove that member composition,
//! rebasing, cross-member overlays, group coordinate space, and per-member
//! baselines fold into one federated graph keyed by `ModuleKey { member, module }`
//! — no test reaches into the per-member composition helpers directly.

use std::collections::BTreeSet;

use toven_engine::config::{CanonicalRegistry, Document, load};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::federation::baseline::MemberVcsReader;
use toven_engine::federation::resolve::PathDriverLocator;
use toven_engine::plan::{NullCache, PlanHost, PlanRequest, Selection, dependency_graph, plan};
use toven_model::{
    AbsPath, DepKind, EcosystemId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace,
    WorkspaceId,
};
use toven_ports::{
    BaselineSpec, ChangeRecord, ChangeStatus, DiscoverResponse, FanOut, Provider, Task, TaskKind,
};
use toven_testkit::workspace::workspace;
use toven_testkit::{
    CountingToolchainProber, FakeConfiguredAdapter, FakeProvider, FakeSourceDigest, FakeVcsReader,
    RecordingReporter,
};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

fn wsid(id: &str) -> WorkspaceId {
    WorkspaceId::new(id).expect("valid workspace id")
}

/// A rust provider discovering a single module `rust:core` at `crates/core`.
fn rust_core_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.workspaces.push(Workspace::new(
        wsid("rust"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    let mut module = Module::new(
        ModuleRef::new(eid("rust"), "core").expect("ref"),
        RepoPath::new("crates/core").expect("root"),
    );
    module.workspace = Some(wsid("rust"));
    response.modules.push(module);
    let task = Task::new(
        TaskKind::Test,
        vec!["cargo".to_string(), "test".to_string()],
        FanOut::WholeWorkspace,
    );
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_tasks(vec![task]);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// A go provider discovering a single module `go:api` at `services/api`.
fn go_api_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("go"));
    response.workspaces.push(Workspace::new(
        wsid("go"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("go"),
    ));
    let mut module = Module::new(
        ModuleRef::new(eid("go"), "api").expect("ref"),
        RepoPath::new("services/api").expect("root"),
    );
    module.workspace = Some(wsid("go"));
    response.modules.push(module);
    let adapter = FakeConfiguredAdapter::new(eid("go")).with_response(response);
    FakeProvider::new(eid("go")).with_adapter(adapter)
}

/// Write the umbrella `toven.toml` and load it into a strict [`Document`].
fn load_umbrella(ws: &toven_testkit::TestWorkspace, body: &str) -> (AbsPath, Document) {
    let path = ws
        .write_file("toven.toml", body.as_bytes())
        .expect("write umbrella");
    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(&path, &BTreeSet::new(), &CanonicalRegistry::model())
        .expect("umbrella loads")
        .document;
    (root, document)
}

#[test]
fn two_members_union_into_distinct_member_scoped_keys() {
    // Two rust members each expose the same `rust:core`. After composition the
    // union must hold two distinct nodes keyed by member, not collapse them.
    let ws = workspace("umbrella-union");
    ws.write_file(
        "repos/core/toven.toml",
        b"[project]\nname = \"core\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("core toml");
    ws.write_file(
        "repos/services/toven.toml",
        b"[project]\nname = \"services\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("services toml");
    let (root, document) = load_umbrella(
        &ws,
        "[project]\nname = \"umbrella\"\n\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n\n[[members]]\nname = \"services\"\nroot = \"repos/services\"\n",
    );

    let rust = rust_core_provider();
    let providers: Vec<&dyn Provider> = vec![&rust];
    let mut reporter = RecordingReporter::new();

    let graph = dependency_graph(
        &root,
        &document,
        &providers,
        &PathDriverLocator::new(),
        &mut reporter,
    )
    .expect("federated graph builds");

    assert_eq!(graph.len(), 2, "each member contributes a distinct node");
    let members: BTreeSet<String> = graph
        .modules()
        .map(|module| module.key().member().expect("member-scoped").to_string())
        .collect();
    assert_eq!(
        members,
        BTreeSet::from(["core".to_string(), "services".to_string()])
    );
    assert!(
        graph
            .modules()
            .all(|module| module.key().module().name == "core"),
        "both members keep the same bare module identity"
    );
}

#[test]
fn umbrella_overlay_links_modules_across_members() {
    // A rust member and a go member, joined by an umbrella overlay from the go
    // module to the rust module, must surface as one cross-member Overlay edge.
    let ws = workspace("umbrella-overlay");
    ws.write_file(
        "repos/core/toven.toml",
        b"[project]\nname = \"core\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("core toml");
    ws.write_file(
        "repos/gateway/toven.toml",
        b"[project]\nname = \"gateway\"\nbase_ref = \"main\"\n[ecosystems.go]\nmodules = [\"api\"]\n",
    )
    .expect("gateway toml");
    let (root, document) = load_umbrella(
        &ws,
        "[project]\nname = \"umbrella\"\n\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n\n[[members]]\nname = \"gateway\"\nroot = \"repos/gateway\"\n\n[[overlays]]\nfrom = { ecosystem = \"go\", module = \"api\" }\nto = { ecosystem = \"rust\", module = \"core\" }\n",
    );

    let rust = rust_core_provider();
    let go = go_api_provider();
    let providers: Vec<&dyn Provider> = vec![&rust, &go];
    let mut reporter = RecordingReporter::new();

    let graph = dependency_graph(
        &root,
        &document,
        &providers,
        &PathDriverLocator::new(),
        &mut reporter,
    )
    .expect("federated graph builds");

    assert_eq!(graph.len(), 2);
    let overlay = graph
        .edges()
        .iter()
        .find(|edge| edge.kind == DepKind::Overlay)
        .expect("an umbrella overlay edge is present");
    assert_eq!(overlay.from.member().expect("scoped").as_str(), "gateway");
    assert_eq!(overlay.from.module().name, "api");
    assert_eq!(overlay.to.member().expect("scoped").as_str(), "core");
    assert_eq!(overlay.to.module().name, "core");
}

#[test]
fn umbrella_group_ambiguous_bare_ref_is_rejected_through_the_front() {
    // Two rust members both expose `rust:core`; an umbrella group naming the bare
    // ref cannot bind it to a single member. The front must validate groups in
    // the composed coordinate space and reject the ambiguity, not silently bind
    // it against the umbrella document.
    let ws = workspace("umbrella-ambiguous-group");
    ws.write_file(
        "repos/core/toven.toml",
        b"[project]\nname = \"core\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("core toml");
    ws.write_file(
        "repos/services/toven.toml",
        b"[project]\nname = \"services\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("services toml");
    let (root, document) = load_umbrella(
        &ws,
        "[project]\nname = \"umbrella\"\n\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n\n[[members]]\nname = \"services\"\nroot = \"repos/services\"\n\n[groups.shared]\nmodules = [\"rust:core\"]\n",
    );

    let rust = rust_core_provider();
    let providers: Vec<&dyn Provider> = vec![&rust];
    let mut reporter = RecordingReporter::new();

    let error = dependency_graph(
        &root,
        &document,
        &providers,
        &PathDriverLocator::new(),
        &mut reporter,
    )
    .expect_err("an ambiguous bare umbrella group ref is a hard error");
    assert!(
        error.to_string().contains("ambiguous") || error.to_string().contains("rust:core"),
        "error should name the ambiguous cross-member ref: {error}"
    );
}

#[test]
fn declared_member_without_toven_toml_is_a_hard_error() {
    // The member directory exists, but it carries no `toven.toml`: a declared
    // member must be a runnable toven project, so this is a typed config error.
    let ws = workspace("umbrella-missing-member-config");
    ws.write_file("repos/core/.keep", b"").expect("placeholder");
    let (root, document) = load_umbrella(
        &ws,
        "[project]\nname = \"umbrella\"\n\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n",
    );

    let rust = rust_core_provider();
    let providers: Vec<&dyn Provider> = vec![&rust];
    let mut reporter = RecordingReporter::new();

    let error = dependency_graph(
        &root,
        &document,
        &providers,
        &PathDriverLocator::new(),
        &mut reporter,
    )
    .expect_err("a member without toven.toml is rejected");
    assert!(
        error.to_string().contains("toven project") || error.to_string().contains("toven.toml"),
        "error should explain the missing member config: {error}"
    );
}

#[test]
fn changed_selection_attributes_changes_to_the_owning_member() {
    // Both members expose the same `rust:core` at the same repo-relative root.
    // A change confined to the core member's repo must, after umbrella-relative
    // prefixing, activate only the core member's module — the services member's
    // identically-named module stays out of the cut.
    let ws = workspace("umbrella-changed");
    ws.write_file(
        "repos/core/toven.toml",
        b"[project]\nname = \"core\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("core toml");
    ws.write_file(
        "repos/services/toven.toml",
        b"[project]\nname = \"services\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("services toml");
    let (root, document) = load_umbrella(
        &ws,
        "[project]\nname = \"umbrella\"\n\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n\n[[members]]\nname = \"services\"\nroot = \"repos/services\"\n",
    );

    let rust = rust_core_provider();
    let providers: Vec<&dyn Provider> = vec![&rust];

    // The core member sees a change; the services member sees none. Each member
    // resolves the same `main` ref name independently against its own repo.
    let core_vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
        "crates/core/src/lib.rs",
        ChangeStatus::Modified,
    )]);
    let services_vcs = FakeVcsReader::new();
    let readers = MemberVcsReaders::new(vec![
        MemberVcsReader::new(
            Some(toven_model::MemberId::new("core").expect("member")),
            "repos/core",
            Some(BaselineSpec::explicit("main")),
            &core_vcs,
        ),
        MemberVcsReader::new(
            Some(toven_model::MemberId::new("services").expect("member")),
            "repos/services",
            Some(BaselineSpec::explicit("main")),
            &services_vcs,
        ),
    ]);

    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let mut reporter = RecordingReporter::new();

    let request = PlanRequest::new("run-1", "umbrella", TaskKind::Test, root)
        .with_selection(Selection::Changed(BaselineSpec::explicit("main")));
    let plan = plan(&request, &document, &providers, host, &mut reporter).expect("plan succeeds");

    assert_eq!(plan.units.len(), 1, "only the changed member is scheduled");
    assert!(
        plan.units[0].id.starts_with("core/"),
        "the scheduled unit belongs to the core member: {}",
        plan.units[0].id
    );
}
