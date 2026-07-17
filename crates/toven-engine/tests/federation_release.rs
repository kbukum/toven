//! End-to-end cross-repo umbrella release sharding test.
//!
//! A two-member umbrella (one rust member, one go member) is driven through the
//! public [`release_run`] facade with per-member VCS doubles. The assertions
//! prove the federated release plan shards its history mutations per member repo
//! — one release commit and its own module tags land in each member's writer —
//! while publishing still runs as one federated pass across both registries.

use std::collections::BTreeSet;

use toven_engine::config::{CanonicalRegistry, Document, load};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::federation::baseline::MemberVcsReader;
use toven_engine::federation::release::{MemberReleaseRepo, MemberReleaseRepos};
use toven_engine::plan::PlanRequest;
use toven_engine::release::{BumpOverrides, ReleaseApplyOptions, release_run};
use toven_model::{
    AbsPath, EcosystemId, MemberId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace,
    WorkspaceId,
};
use toven_ports::{DiscoverResponse, Provider, TaskIntent};
use toven_testkit::workspace::workspace;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, VcsWrite,
};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

fn wsid(id: &str) -> WorkspaceId {
    WorkspaceId::new(id).expect("valid workspace id")
}

/// A publishable provider exposing one module of `ecosystem` at `root`.
fn publishable_provider(
    ecosystem: &str,
    workspace_id: &str,
    module_name: &str,
    module_root: &str,
    toolchain: &str,
) -> FakeProvider {
    let mut response = DiscoverResponse::new(eid(ecosystem));
    response.workspaces.push(Workspace::new(
        wsid(workspace_id),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new(toolchain),
    ));
    let mut module = Module::new(
        ModuleRef::new(eid(ecosystem), module_name).expect("ref"),
        RepoPath::new(module_root).expect("root"),
    );
    module.workspace = Some(wsid(workspace_id));
    response.modules.push(module);
    let adapter = FakeConfiguredAdapter::new(eid(ecosystem))
        .with_response(response)
        .with_release_target(FakeReleaseTarget::new());
    FakeProvider::new(eid(ecosystem)).with_adapter(adapter)
}

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
fn release_shards_history_mutations_per_member_repo() {
    let ws = workspace("umbrella-release-shard");
    ws.write_file(
        "repos/core/toven.toml",
        b"[project]\nname = \"core\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
    )
    .expect("core toml");
    ws.write_file(
        "repos/gateway/toven.toml",
        b"[project]\nname = \"gateway\"\n[ecosystems.go]\nmodules = [\"api\"]\n",
    )
    .expect("gateway toml");
    let (root, document) = load_umbrella(
        &ws,
        "[project]\nname = \"umbrella\"\n\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n\n[[members]]\nname = \"gateway\"\nroot = \"repos/gateway\"\n",
    );

    let rust = publishable_provider("rust", "rust", "core", "crates/core", "cargo");
    let go = publishable_provider("go", "go", "api", "services/api", "go");
    let providers: Vec<&dyn Provider> = vec![&rust, &go];

    // One reader and one writer per member repo; no tags/changes means a first
    // release is planned for every module under a full selection.
    let core_vcs = FakeVcsReader::new();
    let gateway_vcs = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_commit_oid("core-commit");
    let gateway_writer = FakeVcsWriter::new().with_commit_oid("gateway-commit");

    let core_id = MemberId::new("core").expect("member");
    let gateway_id = MemberId::new("gateway").expect("member");
    let readers = MemberVcsReaders::new(vec![
        MemberVcsReader::new(Some(core_id.clone()), "repos/core", None, &core_vcs),
        MemberVcsReader::new(
            Some(gateway_id.clone()),
            "repos/gateway",
            None,
            &gateway_vcs,
        ),
    ]);
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(Some(core_id), &core_vcs, &core_writer),
        MemberReleaseRepo::new(Some(gateway_id), &gateway_vcs, &gateway_writer),
    ]);

    let request = PlanRequest::new("rel-1", "umbrella", TaskIntent::resolve("build"), root);
    let options = ReleaseApplyOptions::default();
    let mut reporter = toven_testkit::RecordingReporter::new();

    let stats = release_run(
        &request,
        &document,
        &providers,
        &readers,
        &repos,
        &BumpOverrides::new(),
        &mut reporter,
        &options,
    )
    .expect("federated release runs");

    // Each member repo gets exactly one release commit and one module tag.
    let core_log = core_writer.writes();
    let gateway_log = gateway_writer.writes();
    assert_eq!(
        core_log
            .iter()
            .filter(|write| matches!(write, VcsWrite::Commit(_)))
            .count(),
        1,
        "the core member commits exactly once: {core_log:?}"
    );
    assert_eq!(
        gateway_log
            .iter()
            .filter(|write| matches!(write, VcsWrite::Commit(_)))
            .count(),
        1,
        "the gateway member commits exactly once: {gateway_log:?}"
    );

    // The core member only tags its rust module; the gateway member only its go
    // module — proving the per-member tag namespace is isolated.
    assert!(
        core_log.iter().any(|write| matches!(
            write,
            VcsWrite::CreateTag { name, .. } if name.contains("core")
        )),
        "core member tags rust:core: {core_log:?}"
    );
    assert!(
        gateway_log.iter().any(|write| matches!(
            write,
            VcsWrite::CreateTag { name, .. } if name.contains("api")
        )),
        "gateway member tags go:api: {gateway_log:?}"
    );
    assert!(
        !core_log.iter().any(|write| matches!(
            write,
            VcsWrite::CreateTag { name, .. } if name.contains("api")
        )),
        "core member must not tag the gateway's module: {core_log:?}"
    );

    // Publishing runs once across both members after the commit boundary.
    assert_eq!(stats.published_modules, 2, "both modules publish federated");
}
