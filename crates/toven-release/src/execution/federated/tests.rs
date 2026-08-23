use rskit_version::semver::Version;
use toven_model::{EcosystemId, MemberId, Module, ModuleKey, ModuleRef, RepoPath};
use toven_ports::ReleaseMutation;
use toven_testkit::{
    FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, RecordingReporter, ReleaseCall, VcsWrite,
};

use super::{
    apply::{MemberReleaseRepo, MemberReleaseRepos},
    release_apply_by_member, release_bump_by_member,
};
use crate::{
    BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseApplyOptions,
    ReleaseEntry, ReleasePlan,
};
use toven_ports::BumpLevel;

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).unwrap()
}

fn mref(name: &str) -> ModuleRef {
    ModuleRef::new(eid("rust"), name).unwrap()
}

fn member(name: &str) -> MemberId {
    MemberId::new(name).unwrap()
}

fn mkey(member: &str, name: &str) -> ModuleKey {
    ModuleKey::new(Some(self::member(member)), mref(name))
}

fn module(member: &str, name: &str) -> Module {
    let mut module = Module::new(
        mref(name),
        RepoPath::new(format!("repos/{member}/crates/{name}")).unwrap(),
    );
    module.member = Some(self::member(member));
    module
}

fn entry(member: &str, name: &str, version: Version, rank: usize) -> ReleaseEntry {
    ReleaseEntry {
        module: mkey(member, name),
        current_version: Some(Version::new(0, 1, 0)),
        planned_version: Some(version.clone()),
        planned_tag: Some(format!("rust/{name}@{version}")),
        level: BumpLevel::Patch,
        reason: BumpReason::Changed,
        winning_input: BumpSource::Default,
        cascade_origin: None,
        prerelease_channel: None,
        up_to_date: false,
        mutation: ReleaseMutation::version(version),
        publication: toven_ports::PublicationPolicy::Registry {
            registry: "crates-io".into(),
        },
        publish_needed: true,
        tag_format: None,
        tag_mode: None,
        baseline_source: None,
        tag_message: None,
        signer: None,
        commit_message: None,
        token_env: None,
        visibility: toven_ports::Visibility::Public,
        push: PushPolicy::BranchAndTags,
        remote: "origin".into(),
        branches: Vec::new(),
        topo_rank: rank,
        baseline: None,
        changelog: ChangelogEntry::new(mkey(member, name), "changed", Vec::new()),
        changelog_path: "CHANGELOG.md".into(),
        changelog_roll: false,
        entrypoint: toven_model::Entrypoint::Toven,
        umbrella: false,
        version_references: Vec::new(),
        on_resolved: Vec::new(),
    }
}

fn targets(target: &FakeReleaseTarget) -> crate::ReleaseTargets {
    let mut map = crate::ReleaseTargets::new();
    // Every member in these fixtures exposes the same publishable `rust` target.
    for owner in ["core", "gateway"] {
        map.insert((Some(member(owner)), eid("rust")), Box::new(target.clone()));
    }
    map
}

fn targets_by_member(
    core: &FakeReleaseTarget,
    gateway: &FakeReleaseTarget,
) -> crate::ReleaseTargets {
    let mut map = crate::ReleaseTargets::new();
    map.insert((Some(member("core")), eid("rust")), Box::new(core.clone()));
    map.insert(
        (Some(member("gateway")), eid("rust")),
        Box::new(gateway.clone()),
    );
    map
}

#[test]
fn resolved_version_map_keeps_every_module_and_drops_ambiguous_aliases() {
    use std::collections::BTreeMap;

    use super::hooks::resolved_version_map;

    // Two federation members each expose `rust:core`: the bare name `core`
    // and the `ecosystem:name` `rust:core` are ambiguous across members,
    // while each member bumps to a distinct version.
    let billing = entry("billing", "core", Version::new(1, 0, 0), 0);
    let gateway = entry("gateway", "core", Version::new(2, 0, 0), 1);
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![billing, gateway]);
    let modules = [module("billing", "core"), module("gateway", "core")];
    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();

    let map = resolved_version_map(&plan, &module_by_ref);

    // Every module stays reachable by its canonical member-qualified key,
    // with its own version — neither is dropped or shadowed.
    assert_eq!(
        map.get("billing/rust:core"),
        Some(&Version::new(1, 0, 0)),
        "billing's canonical key resolves to its own version: {map:?}"
    );
    assert_eq!(
        map.get("gateway/rust:core"),
        Some(&Version::new(2, 0, 0)),
        "gateway's canonical key resolves to its own version: {map:?}"
    );
    // The ambiguous aliases are omitted rather than resolving to a wrong,
    // last-writer-wins version.
    assert!(
        !map.contains_key("core") && !map.contains_key("rust:core"),
        "an alias two members share is dropped, not silently overwritten: {map:?}"
    );
}

#[test]
fn resolved_version_map_drops_aliases_colliding_with_versionless_entries() {
    use std::collections::BTreeMap;

    use super::hooks::resolved_version_map;

    // Member `billing` has a planned version for `rust:core`, while member
    // `gateway` has a versionless entry (e.g. tagless floor-only upgrade) for
    // `rust:core`. The shared aliases (`core`, `rust:core`) collide and must be
    // omitted, rather than resolving to billing's version.
    let billing = entry("billing", "core", Version::new(1, 0, 0), 0);
    let mut gateway = entry("gateway", "core", Version::new(2, 0, 0), 1);
    gateway.current_version = None;
    gateway.planned_version = None;
    gateway.mutation.new_version = None;

    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![billing, gateway]);
    let modules = [module("billing", "core"), module("gateway", "core")];
    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();

    let map = resolved_version_map(&plan, &module_by_ref);

    assert_eq!(
        map.get("billing/rust:core"),
        Some(&Version::new(1, 0, 0)),
        "billing's canonical key resolves to its own version: {map:?}"
    );
    assert!(
        !map.contains_key("gateway/rust:core"),
        "gateway is versionless so it has no canonical version entry: {map:?}"
    );
    assert!(
        !map.contains_key("core") && !map.contains_key("rust:core"),
        "an alias shared with a versionless entry is dropped, not leaked to billing: {map:?}"
    );
}

#[test]
fn resolved_version_map_keeps_unambiguous_aliases() {
    use std::collections::BTreeMap;

    use super::hooks::resolved_version_map;

    // A single module owns `core`/`rust:core` outright, so both aliases stay
    // alongside its canonical key.
    let shared = entry("billing", "core", Version::new(1, 2, 3), 0);
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![shared]);
    let modules = [module("billing", "core")];
    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();

    let map = resolved_version_map(&plan, &module_by_ref);

    for key in ["billing/rust:core", "rust:core", "core"] {
        assert_eq!(
            map.get(key),
            Some(&Version::new(1, 2, 3)),
            "an unambiguous module is reachable by '{key}': {map:?}"
        );
    }
}

#[test]
fn an_all_member_tags_exist_release_resumes_without_git_mutation() {
    // Every member's planned tag already exists: the commits, tags, and
    // pushes happened on a prior attempt, so APPLY resumes across the
    // federation — no member mutates, commits, tags, or pushes — and the
    // already-published versions make the publish loop a clean no-op.
    let mut core_entry = entry("core", "shared", Version::new(0, 1, 1), 0);
    core_entry.publish_needed = false;
    core_entry.publication = toven_ports::PublicationPolicy::TagOnly;
    let mut gateway_entry = entry("gateway", "api", Version::new(0, 1, 1), 1);
    gateway_entry.publish_needed = false;
    gateway_entry.publication = toven_ports::PublicationPolicy::TagOnly;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core_entry, gateway_entry]);
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
        "rust/shared@0.1.1",
        toven_ports::Oid::new("deadbee"),
    )]);
    let gateway_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
        "rust/api@0.1.1",
        toven_ports::Oid::new("cafef00d"),
    )]);
    let core_writer = FakeVcsWriter::new();
    let gateway_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let stats = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect("an already-tagged federation resumes rather than failing closed");

    assert!(stats.resumed, "the run is marked resumed");
    assert_eq!(stats.tagged_modules, 0);
    assert!(
        core_writer.writes().is_empty() && gateway_writer.writes().is_empty(),
        "no member may commit/tag/push on resume: core={:?} gateway={:?}",
        core_writer.writes(),
        gateway_writer.writes()
    );
    assert!(
        !target.calls().iter().any(|call| matches!(
            call,
            ReleaseCall::ApplyRelease { .. } | ReleaseCall::Package(_) | ReleaseCall::Publish(_)
        )),
        "no manifest mutation, packaging, or publish may happen on resume: {:?}",
        target.calls()
    );
}

#[test]
fn a_partial_planned_tag_set_in_a_member_is_rejected_before_any_mutation() {
    // One member owns two distinct tags; only one exists — a partial overlap
    // within the member's own tag train is an interrupted or divergent
    // release, not a resume, and fails closed before any mutation.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("core", "extra", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("core", "extra")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
        "rust/shared@0.1.1",
        toven_ports::Oid::new("deadbee"),
    )]);
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a partial tag overlap must fail closed before any mutation");

    let message = error.to_string();
    assert!(message.contains("immutable"), "{message}");
    assert!(message.contains("forward-fix"), "{message}");
    assert!(
        core_writer.writes().is_empty(),
        "no write may reach the member on a partial overlap"
    );
}

#[test]
fn a_tag_failure_after_a_member_commit_carries_forward_only_recovery_guidance() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", "shared", Version::new(0, 1, 1), 0)],
    );
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_create_tag_failure("tag rejected");
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a post-commit tag failure must surface recovery guidance");

    let message = error.to_string();
    assert!(message.contains("tagging"), "{message}");
    assert!(message.contains("tag rejected"), "{message}");
    assert!(message.contains("member 'core'"), "{message}");
    assert!(message.contains("toven release status"), "{message}");
    assert!(message.contains("forward fix"), "{message}");
    // The commit is past the rollback boundary: no worktree restore.
    assert!(
        core_writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Commit { .. }))
    );
    assert!(
        !core_writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree))
    );
}

#[test]
fn a_push_failure_after_the_member_commits_carries_forward_only_recovery_guidance() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", "shared", Version::new(0, 1, 1), 0)],
    );
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_push_failure("remote rejected");
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect_err("a post-commit push failure must surface recovery guidance");

    let message = error.to_string();
    assert!(message.contains("push"), "{message}");
    assert!(message.contains("remote rejected"), "{message}");
    assert!(message.contains("member 'core'"), "{message}");
    assert!(message.contains("toven release status"), "{message}");
    assert!(message.contains("forward fix"), "{message}");
    // The commit and tag are past the rollback boundary: no worktree
    // restore may be attempted for a post-commit push failure.
    assert!(
        core_writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Commit { .. }))
    );
    assert!(
        !core_writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree))
    );
}

#[test]
fn tags_only_member_push_proceeds_on_a_detached_head() {
    let mut shared = entry("core", "shared", Version::new(0, 1, 1), 0);
    shared.push = PushPolicy::TagsOnly;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![shared]);
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new().with_detached_head();
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("a tags-only push does not require a checked-out branch");

    let (_, refspecs) = core_writer
        .writes()
        .into_iter()
        .find_map(|w| match w {
            VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
            _ => None,
        })
        .expect("push recorded");
    assert_eq!(refspecs, vec!["refs/tags/rust/shared@0.1.1".to_string()]);
}

#[test]
fn a_publish_failure_after_the_member_commits_carries_forward_only_recovery_guidance() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", "shared", Version::new(0, 1, 1), 0)],
    );
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new().with_publish_failure("registry unavailable");
    let core_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a post-commit publish failure must surface recovery guidance");

    let message = error.to_string();
    assert!(message.contains("publication"), "{message}");
    assert!(message.contains("registry unavailable"), "{message}");
    assert!(message.contains("toven release status"), "{message}");
    assert!(message.contains("forward fix"), "{message}");
    assert!(
        !core_writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree))
    );
}

#[test]
fn release_apply_commits_per_member_and_publishes_in_federated_order() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
    let gateway_writer = FakeVcsWriter::new().with_commit_oid("gwcommit");
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let stats = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .unwrap();

    assert_eq!(stats.mutated_modules, 2);
    assert_eq!(stats.tagged_modules, 2);
    assert_eq!(stats.published_modules, 2);
    assert!(matches!(
        &core_writer.writes()[0],
        VcsWrite::Commit { message, paths }
            if message == "release: rust/shared@0.1.1"
                && paths == &vec!["repos/core/crates/shared".to_string()]
    ));
    assert!(matches!(
        &gateway_writer.writes()[0],
        VcsWrite::Commit { message, paths }
            if message == "release: rust/api@0.1.1"
                && paths == &vec!["repos/gateway/crates/api".to_string()]
    ));
    assert!(matches!(
        &core_writer.writes()[1],
        VcsWrite::CreateTag { name, target_rev, .. }
            if name == "rust/shared@0.1.1" && target_rev == "corecommit"
    ));
    assert!(matches!(
        &gateway_writer.writes()[1],
        VcsWrite::CreateTag { name, target_rev, .. }
            if name == "rust/api@0.1.1" && target_rev == "gwcommit"
    ));

    let published = target
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            ReleaseCall::Publish(module) => Some(module.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(published, vec!["rust:shared", "rust:api"]);
}

#[test]
fn member_prepare_failure_restores_only_that_member() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new().with_package_failure("package failed");
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new();
    let gateway_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("package failed"));
    assert_eq!(core_writer.writes(), vec![VcsWrite::RestoreWorktree]);
    assert!(gateway_writer.writes().is_empty());
}

#[test]
fn later_member_prepare_failure_restores_prepared_members_before_any_commit() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let core_target = FakeReleaseTarget::new();
    let gateway_target = FakeReleaseTarget::new().with_package_failure("gateway package failed");
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
    let gateway_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets_by_member(&core_target, &gateway_target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("gateway package failed"));
    assert_eq!(core_writer.writes(), vec![VcsWrite::RestoreWorktree]);
    assert_eq!(gateway_writer.writes(), vec![VcsWrite::RestoreWorktree]);
}

#[test]
fn member_without_a_release_target_is_not_served_by_another_members_target() {
    // `core` publishes `rust`; `gateway` does not (its rust adapter is `publish =
    // false`, so it contributes no target). Keying targets by `(member, ecosystem)`
    // must not let `gateway`'s module borrow `core`'s target and get silently
    // released.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let mut targets = crate::ReleaseTargets::new();
    targets.insert(
        (Some(member("core")), eid("rust")),
        Box::new(FakeReleaseTarget::new()),
    );
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
    let gateway_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets,
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("has no release target"));
    assert!(gateway_writer.writes().is_empty());
}

#[test]
fn a_maintainer_owned_member_publishes_against_the_existing_tag_without_mutating() {
    // The maintainer already created the tag `rust/shared@0.2.0` (and the
    // hosted Release) in the forge. A maintainer-owned member shard verifies
    // that tag, then publishes the version the registry still lacks against
    // it — mutating no manifest and creating no commit, tag, or push. This
    // drives the production `release_run` path (`release_apply_by_member`),
    // not the standalone `release_apply`.
    let mut owned = entry("core", "shared", Version::new(0, 2, 0), 0);
    owned.entrypoint = toven_model::Entrypoint::Maintainer;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![owned]);
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new()
        .with_rev_parse("deadbee")
        .with_tags(vec![toven_ports::TagRef::new(
            "rust/shared@0.2.0",
            toven_ports::Oid::new("deadbee"),
        )]);
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let stats = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect("a maintainer-owned member publishes against the existing tag");

    assert_eq!(stats.published_modules, 1);
    assert_eq!(stats.tagged_modules, 0);
    assert_eq!(stats.mutated_modules, 0);
    assert!(
        core_writer.writes().is_empty(),
        "no commit/tag/push may happen for a maintainer-owned member: {:?}",
        core_writer.writes()
    );
    assert!(
        !target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::ApplyRelease { .. })),
        "a maintainer-owned member never mutates a manifest: {:?}",
        target.calls()
    );
    assert!(
        target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::Publish(_))),
        "a maintainer-owned member publishes against the existing tag: {:?}",
        target.calls()
    );
}

#[test]
fn a_maintainer_owned_member_fails_closed_when_the_release_tag_is_absent() {
    // No tag exists for the planned version: the maintainer has not cut the
    // Release yet, or the manifest version and the created tag diverge. The
    // production `release_apply_by_member` path refuses to proceed rather
    // than creating or moving the tag itself — zero VCS writes, no mutation.
    let mut owned = entry("core", "shared", Version::new(0, 2, 0), 0);
    owned.entrypoint = toven_model::Entrypoint::Maintainer;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![owned]);
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect_err("an absent maintainer-owned tag fails closed");

    let message = error.to_string();
    assert!(message.contains("rust/shared@0.2.0"), "{message}");
    assert!(message.contains("never creates or moves"), "{message}");
    assert!(
        core_writer.writes().is_empty(),
        "no VCS write may happen when a maintainer-owned tag is absent: {:?}",
        core_writer.writes()
    );
    assert!(
        !target.calls().iter().any(|call| matches!(
            call,
            ReleaseCall::ApplyRelease { .. } | ReleaseCall::Publish(_)
        )),
        "no mutation or publish may happen when the tag is absent: {:?}",
        target.calls()
    );
}

#[test]
fn a_maintainer_owned_member_fails_closed_when_the_tag_diverges_from_head() {
    // The member's tag exists but points at an earlier commit than the
    // checked-out HEAD. The production `release_apply_by_member` path
    // packages and publishes from HEAD, so it fails closed rather than
    // attaching artifacts to a tag that names a different commit — zero VCS
    // writes, no publish.
    let mut owned = entry("core", "shared", Version::new(0, 2, 0), 0);
    owned.entrypoint = toven_model::Entrypoint::Maintainer;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![owned]);
    let modules = vec![module("core", "shared")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new()
        .with_rev_parse("cafef00d")
        .with_tags(vec![toven_ports::TagRef::new(
            "rust/shared@0.2.0",
            toven_ports::Oid::new("deadbee"),
        )]);
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);

    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut RecordingReporter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a maintainer-owned tag that diverges from HEAD fails closed");

    let message = error.to_string();
    assert!(message.contains("rust/shared@0.2.0"), "{message}");
    assert!(message.contains("checked-out HEAD"), "{message}");
    assert!(
        core_writer.writes().is_empty(),
        "no VCS write may happen when a maintainer-owned tag diverges: {:?}",
        core_writer.writes()
    );
    assert!(
        !target.calls().iter().any(|call| matches!(
            call,
            ReleaseCall::ApplyRelease { .. } | ReleaseCall::Publish(_)
        )),
        "no mutation or publish may happen when the tag diverges: {:?}",
        target.calls()
    );
}

/// Extract the ordered `(module, new_version, tag)` triples from every
/// `ModuleReleaseStaged` commit event a run recorded. `new_version` is `None`
/// for a dependency-floor-only stage.
fn staged(reporter: &RecordingReporter) -> Vec<(String, Option<String>, Option<String>)> {
    reporter
        .events()
        .iter()
        .filter_map(|event| match event {
            toven_model::Event::ModuleReleaseStaged {
                module,
                new_version,
                tag,
                ..
            } => Some((module.clone(), new_version.clone(), tag.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn release_apply_streams_a_staged_event_per_committed_module_after_its_tag_lands() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
    let gateway_writer = FakeVcsWriter::new().with_commit_oid("gwcommit");
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let mut reporter = RecordingReporter::new();
    release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut reporter,
        &ReleaseApplyOptions::default(),
    )
    .unwrap();

    // One commit event per module, in plan order, each carrying the tag its
    // Phase-2 side effect actually created.
    assert_eq!(
        staged(&reporter),
        vec![
            (
                "core/rust:shared".to_string(),
                Some("0.1.1".to_string()),
                Some("rust/shared@0.1.1".to_string()),
            ),
            (
                "gateway/rust:api".to_string(),
                Some("0.1.1".to_string()),
                Some("rust/api@0.1.1".to_string()),
            ),
        ],
    );
}

#[test]
fn a_member_commit_failure_emits_no_staged_event_for_the_failing_member() {
    // The gateway member's tag creation fails after the core member has already
    // committed and tagged. The core member's commit landed, so its staged event
    // is truthful; the gateway member never completed its Phase-2 side effect, so
    // it must emit no staged event and the run fails closed.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
    let gateway_writer = FakeVcsWriter::new()
        .with_commit_oid("gwcommit")
        .with_create_tag_failure("gateway tag failed");
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);

    let mut reporter = RecordingReporter::new();
    let error = release_apply_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        &mut reporter,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a member tag failure fails the run closed");

    assert!(error.to_string().contains("gateway tag failed"), "{error}");
    assert_eq!(
        staged(&reporter),
        vec![(
            "core/rust:shared".to_string(),
            Some("0.1.1".to_string()),
            Some("rust/shared@0.1.1".to_string()),
        )],
        "only the genuinely-committed core member may emit a staged event",
    );
}

#[test]
fn release_bump_streams_a_staged_event_per_genuinely_staged_module() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new();
    let gateway_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);
    let hooks = toven_testkit::RecordingHookRunner::new();

    let mut reporter = RecordingReporter::new();
    let report = release_bump_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        "2026-01-01",
        &hooks,
        &mut reporter,
        crate::BumpOptions::default(),
    )
    .unwrap();

    assert!(report.staged);
    // `bump` stages the mutation but creates no tag, so every commit event names
    // a version and no tag, in plan order.
    assert_eq!(
        staged(&reporter),
        vec![
            (
                "core/rust:shared".to_string(),
                Some("0.1.1".to_string()),
                None
            ),
            (
                "gateway/rust:api".to_string(),
                Some("0.1.1".to_string()),
                None
            ),
        ],
    );
    // Each staged event carries the manifests the mutation actually rewrote.
    let manifests: Vec<Vec<String>> = reporter
        .events()
        .iter()
        .filter_map(|event| match event {
            toven_model::Event::ModuleReleaseStaged { manifests, .. } => Some(manifests.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        manifests,
        vec![
            vec!["repos/core/crates/shared".to_string()],
            vec!["repos/gateway/crates/api".to_string()],
        ],
    );
}

#[test]
fn release_bump_emits_no_staged_event_when_a_mutation_fails_mid_transaction() {
    // A Phase-1 mutation failure restores every already-mutated member and stages
    // nothing, so no module may surface a rolled-back "staged" commit event.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", "shared", Version::new(0, 1, 1), 0),
            entry("gateway", "api", Version::new(0, 1, 1), 1),
        ],
    );
    let modules = vec![module("core", "shared"), module("gateway", "api")];
    let target = FakeReleaseTarget::new().with_apply_failure("manifest mutation failed");
    let core_reader = FakeVcsReader::new();
    let gateway_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new();
    let gateway_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![
        MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        ),
        MemberReleaseRepo::new(
            Some(member("gateway")),
            std::path::PathBuf::from("/repos/gateway"),
            &gateway_reader,
            &gateway_writer,
        ),
    ]);
    let hooks = toven_testkit::RecordingHookRunner::new();

    let mut reporter = RecordingReporter::new();
    let error = release_bump_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        "2026-01-01",
        &hooks,
        &mut reporter,
        crate::BumpOptions::default(),
    )
    .expect_err("a mid-transaction mutation failure fails the bump closed");

    assert!(
        error.to_string().contains("manifest mutation failed"),
        "{error}"
    );
    assert!(
        staged(&reporter).is_empty(),
        "a restored member must emit no staged event: {:?}",
        reporter.events()
    );
}

#[test]
fn release_bump_streams_a_staged_event_for_a_dependency_floor_only_module() {
    // `app` receives no own-version bump but re-floors its dependency on `shared`,
    // so its manifest is genuinely rewritten and staged. It must surface a staged
    // event carrying no `new_version` (and no tag) rather than being filtered out.
    let mut app = entry("core", "app", Version::new(0, 1, 1), 1);
    app.planned_version = None;
    app.planned_tag = None;
    app.mutation = ReleaseMutation {
        new_version: None,
        dep_floor_updates: std::collections::BTreeMap::from([(
            mref("shared"),
            Version::new(0, 1, 1),
        )]),
        dep_floor_import_updates: std::collections::BTreeMap::new(),
    };
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", "shared", Version::new(0, 1, 1), 0), app],
    );
    let modules = vec![module("core", "shared"), module("core", "app")];
    let target = FakeReleaseTarget::new();
    let core_reader = FakeVcsReader::new();
    let core_writer = FakeVcsWriter::new();
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        Some(member("core")),
        std::path::PathBuf::from("/repos/core"),
        &core_reader,
        &core_writer,
    )]);
    let hooks = toven_testkit::RecordingHookRunner::new();

    let mut reporter = RecordingReporter::new();
    let report = release_bump_by_member(
        &plan,
        &modules,
        &targets(&target),
        &repos,
        "2026-01-01",
        &hooks,
        &mut reporter,
        crate::BumpOptions::default(),
    )
    .unwrap();

    assert!(report.staged);
    // The floor-only module emits a staged event with no version and no tag,
    // alongside the own-version bump — one record per landed module.
    assert_eq!(
        staged(&reporter),
        vec![
            (
                "core/rust:shared".to_string(),
                Some("0.1.1".to_string()),
                None
            ),
            ("core/rust:app".to_string(), None, None),
        ],
    );
}
