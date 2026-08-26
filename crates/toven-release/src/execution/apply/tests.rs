use rskit_errors::ErrorCode;
use rskit_version::semver::Version;
use toven_model::{EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
use toven_ports::{
    ChangeRecord, ChangeStatus, Oid, PublishOutcome, ReleaseMutation, TagRef, TagScheme,
};
use toven_testkit::{FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, ReleaseCall, VcsWrite};

use super::{
    options::{ReleaseApplyOptions, reconcile_repo_settings},
    orchestration::release_apply,
    staging::stage_and_commit,
};
use crate::{
    BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseEntry, ReleasePlan,
};
use toven_ports::BumpLevel;

fn mref(name: &str) -> ModuleRef {
    ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
}

fn mkey(name: &str) -> ModuleKey {
    ModuleKey::bare(mref(name))
}

fn module(name: &str) -> Module {
    let mut module = Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap());
    module.manifest = Some(RepoPath::new(format!("crates/{name}/Cargo.toml")).unwrap());
    module
}

fn entry(name: &str, version: Version, publish_needed: bool, rank: usize) -> ReleaseEntry {
    ReleaseEntry {
        module: mkey(name),
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
        publication: if publish_needed {
            toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into(),
            }
        } else {
            toven_ports::PublicationPolicy::TagOnly
        },
        publish_needed,
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
        changelog: ChangelogEntry::new(mkey(name), "changed", Vec::new()),
        changelog_path: "CHANGELOG.md".into(),
        changelog_roll: false,
        entrypoint: toven_model::Entrypoint::Toven,
        umbrella: false,
        version_references: Vec::new(),
        on_resolved: Vec::new(),
    }
}

fn targets(pairs: Vec<(&str, FakeReleaseTarget)>) -> crate::ReleaseTargets {
    // All fixtures use a single single-repo `rust` ecosystem.
    let mut map = crate::ReleaseTargets::new();
    let (_, target) = pairs.into_iter().next().expect("at least one target");
    map.insert((None, EcosystemId::new("rust").unwrap()), Box::new(target));
    map
}

fn dirty() -> FakeVcsReader {
    FakeVcsReader::new()
        .with_worktree_status(vec![ChangeRecord::new("go.sum", ChangeStatus::Modified)])
}

#[cfg(unix)]
#[test]
fn stage_and_commit_rejects_non_utf8_repo_paths() {
    use std::os::unix::ffi::OsStringExt;

    let path = RepoPath::new(std::path::PathBuf::from(std::ffi::OsString::from_vec(
        vec![b'b', b'a', b'd', 0xff],
    )))
    .expect("repo-relative path");
    let writer = FakeVcsWriter::new();

    let error = stage_and_commit(&writer, &[path], "release").expect_err("non-UTF-8 path");

    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(error.to_string().contains("non-UTF-8 repo path"));
    assert!(
        writer.writes().is_empty(),
        "invalid path must fail before staging"
    );
}

#[test]
fn applies_mutations_commits_tags_and_publishes_in_order() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", Version::new(0, 1, 1), true, 0),
            entry("app", Version::new(0, 1, 1), true, 1),
        ],
    );
    let modules = vec![module("core"), module("app")];
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

    let stats = release_apply(
        &plan,
        &modules,
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("release apply");

    assert_eq!(stats.mutated_modules, 2);
    assert_eq!(stats.packaged_artifacts, 2);
    assert_eq!(stats.tagged_modules, 2);
    assert_eq!(stats.published_modules, 2);
    assert_eq!(stats.skipped_published_modules, 0);

    let recorded = writer.writes();
    assert_eq!(
        recorded[0],
        VcsWrite::Commit {
            message: "release: rust/core@0.1.1, rust/app@0.1.1".into(),
            paths: vec![
                "crates/core/Cargo.toml".into(),
                "crates/app/Cargo.toml".into(),
            ],
        }
    );
    assert!(matches!(
        &recorded[1],
        VcsWrite::CreateTag { name, target_rev, .. } if name == "rust/core@0.1.1" && target_rev == "c0ffee"
    ));
    assert!(matches!(&recorded[2], VcsWrite::CreateTag { name, .. } if name == "rust/app@0.1.1"));
    assert!(!recorded.iter().any(|w| matches!(w, VcsWrite::Push { .. })));

    // Publish happens after the commit/tag writes (apply -> package -> publish).
    let calls = target.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| matches!(c, ReleaseCall::ApplyRelease { .. }))
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| matches!(c, ReleaseCall::Publish(_)))
            .count(),
        2
    );
}

#[test]
fn a_configured_token_env_is_threaded_to_the_publish_target() {
    // The resolved token_env rides from the plan entry to the release target
    // as the credential context — proving publish-time credential injection
    // is wired end-to-end (the target reads only the variable name).
    let mut core = entry("core", Version::new(0, 1, 1), true, 0);
    core.token_env = Some("CARGO_REGISTRY_TOKEN".into());
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core]);
    let target = FakeReleaseTarget::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new().with_commit_oid("c0ffee"),
        &ReleaseApplyOptions::default(),
    )
    .expect("release apply");

    assert_eq!(
        target.publish_token_envs(),
        vec![Some("CARGO_REGISTRY_TOKEN".to_string())],
        "the publish target must receive the resolved token_env as its credential context"
    );
}

#[test]
fn the_default_registry_threads_to_the_publish_target_unchanged() {
    // The default `crates-io` publication registry rides to the target as
    // `Some("crates-io")`; the rust adapter maps that to cargo's default
    // registry, so crates.io publishing is unchanged.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new().with_commit_oid("c0ffee"),
        &ReleaseApplyOptions::default(),
    )
    .expect("release apply");

    assert_eq!(
        target.publish_registries(),
        vec![Some("crates-io".to_string())]
    );
}

#[test]
fn a_named_registry_is_threaded_to_the_publish_target() {
    // A non-crates.io publication registry rides from the plan entry through
    // the publish loop to the target, proving the generic-registry path is
    // wired end-to-end (the adapter routes `--registry` from this name).
    let mut core = entry("core", Version::new(0, 1, 1), true, 0);
    core.publication = toven_ports::PublicationPolicy::Registry {
        registry: "my-corp".into(),
    };
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core]);
    let target = FakeReleaseTarget::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new().with_commit_oid("c0ffee"),
        &ReleaseApplyOptions::default(),
    )
    .expect("release apply");

    assert_eq!(
        target.publish_registries(),
        vec![Some("my-corp".to_string())],
        "the publish target must receive the resolved registry as its credential context"
    );
}

#[test]
fn resolved_visibility_is_threaded_to_the_publish_target() {
    // A non-default visibility rides from the plan entry through the publish
    // loop to the release target, proving the exposure reaches the registry
    // mutation boundary (the target-side fail-closed check consumes it).
    let mut core = entry("core", Version::new(0, 1, 1), true, 0);
    core.visibility = toven_ports::Visibility::Internal;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core]);
    let target = FakeReleaseTarget::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new().with_commit_oid("c0ffee"),
        &ReleaseApplyOptions::default(),
    )
    .expect("release apply");

    assert_eq!(
        target.publish_visibilities(),
        vec![toven_ports::Visibility::Internal],
        "the publish target must receive the resolved visibility as the release exposure"
    );
}

#[test]
fn default_visibility_reaches_the_publish_target_as_public() {
    // A module that omits `visibility` releases public, unchanged: the entry
    // default threads through as `Public`.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new().with_commit_oid("c0ffee"),
        &ReleaseApplyOptions::default(),
    )
    .expect("release apply");

    assert_eq!(
        target.publish_visibilities(),
        vec![toven_ports::Visibility::Public],
    );
}

#[test]
fn push_emits_commit_and_tag_refspecs() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(1, 0, 0), true, 0)],
    );
    let writer = FakeVcsWriter::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("release apply with push");

    let (remote, push) = writer
        .writes()
        .into_iter()
        .find_map(|w| match w {
            VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
            _ => None,
        })
        .expect("push recorded");
    assert_eq!(remote, "origin");
    assert_eq!(
        push,
        vec![
            "refs/heads/main".to_string(),
            "refs/tags/rust/core@1.0.0".to_string()
        ]
    );
}

#[test]
fn tags_only_push_policy_pushes_tags_only() {
    let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
    entry.push = PushPolicy::TagsOnly;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]);
    let writer = FakeVcsWriter::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("release apply with tags-only push");

    let (_, push) = writer
        .writes()
        .into_iter()
        .find_map(|w| match w {
            VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
            _ => None,
        })
        .expect("push recorded");
    assert_eq!(push, vec!["refs/tags/rust/core@1.0.0".to_string()]);
}

#[test]
fn push_refspecs_omits_the_branch_when_no_branch_is_pushed() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(1, 0, 0), true, 0)],
    );

    let with_branch = super::push_refspecs(&plan, Some("main")).expect("refspecs");
    assert_eq!(with_branch[0], "refs/heads/main");

    let tags_only = super::push_refspecs(&plan, None).expect("refspecs");
    assert!(
        tags_only.iter().all(|spec| spec.starts_with("refs/tags/")),
        "{tags_only:?}"
    );
    assert_eq!(tags_only, vec!["refs/tags/rust/core@1.0.0".to_string()]);
}

#[test]
fn tags_only_push_proceeds_on_a_detached_head() {
    let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
    entry.push = PushPolicy::TagsOnly;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]);
    let writer = FakeVcsWriter::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new().with_detached_head(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("a tags-only push does not require a checked-out branch");

    let (_, push) = writer
        .writes()
        .into_iter()
        .find_map(|w| match w {
            VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
            _ => None,
        })
        .expect("push recorded");
    assert_eq!(push, vec!["refs/tags/rust/core@1.0.0".to_string()]);
}

#[test]
fn branch_push_still_requires_a_checked_out_branch() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(1, 0, 0), true, 0)],
    );
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new().with_detached_head(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect_err("pushing the branch requires resolving one");

    assert!(error.to_string().contains("detached"), "{error}");
}

#[test]
fn reconcile_rejects_conflicting_push_policies() {
    let first = entry("core", Version::new(1, 0, 0), true, 0);
    let mut second = entry("util", Version::new(1, 0, 0), true, 1);
    second.push = PushPolicy::TagsOnly;

    let error = reconcile_repo_settings(&[first, second]).expect_err("conflict rejected");
    assert!(error.to_string().contains("push"), "{error}");
}

#[test]
fn reconcile_rejects_mixed_entrypoints_in_one_repository() {
    // A single member shard must be wholly maintainer-owned or wholly
    // Toven-owned: mixing entrypoints within one repository would leave the
    // shard's mutation set ambiguous (verify-only vs commit/tag/push), so it
    // fails closed before any member acts.
    let first = entry("core", Version::new(1, 0, 0), true, 0);
    let mut second = entry("util", Version::new(1, 0, 0), true, 1);
    second.entrypoint = toven_model::Entrypoint::Maintainer;

    let error = reconcile_repo_settings(&[first, second]).expect_err("conflict rejected");
    assert!(error.to_string().contains("entrypoint"), "{error}");
}

#[test]
fn configured_remote_and_push_gate_control_the_member_push() {
    let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
    entry.remote = "release".into();
    entry.push = PushPolicy::Disabled;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry.clone()]);
    let writer = FakeVcsWriter::new();

    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("config-gated local release");
    assert!(
        !writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Push { .. }))
    );

    entry.push = PushPolicy::BranchAndTags;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]);
    let writer = FakeVcsWriter::new();
    release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("configured remote push");
    assert!(writer.writes().iter().any(|write| matches!(
        write,
        VcsWrite::Push { remote, .. } if remote == "release"
    )));
}

#[test]
fn branch_restriction_rejects_before_any_release_write() {
    let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
    entry.branches = vec!["release".into()];
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]),
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new().with_current_branch("main"),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("disallowed branch");

    assert!(error.to_string().contains("release.branches"));
    assert!(writer.writes().is_empty());
}

/// Created-tag names recorded by a `FakeVcsWriter`, in order.
fn created_tag_names(writer: &FakeVcsWriter) -> Vec<String> {
    writer
        .writes()
        .into_iter()
        .filter_map(|w| match w {
            VcsWrite::CreateTag { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

/// An umbrella entry (`suite`, `v{version}`) plus a per-module entry
/// (`core`, `rust/core@…`) sharing a train, with the given tag mode.
fn umbrella_and_per_module(mode: toven_ports::TagMode) -> ReleasePlan {
    let mut suite = entry("suite", Version::new(0, 2, 0), true, 0);
    suite.tag_format = Some("v{version}".into());
    suite.planned_tag = Some("v0.2.0".into());
    suite.umbrella = true;
    suite.tag_mode = Some(mode);
    let mut core = entry("core", Version::new(0, 2, 0), true, 1);
    core.tag_mode = Some(mode);
    ReleasePlan::new(BumpPolicy::SemverCascade, vec![suite, core])
}

fn apply_umbrella_train(plan: &ReleasePlan, writer: &FakeVcsWriter) {
    release_apply(
        plan,
        &[module("suite"), module("core")],
        &targets(vec![("suite", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("umbrella train applies");
}

#[test]
fn per_module_tag_mode_creates_only_per_module_tags() {
    let plan = umbrella_and_per_module(toven_ports::TagMode::PerModule);
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");
    apply_umbrella_train(&plan, &writer);
    // The umbrella module's `v0.2.0` tag is skipped; only the per-module tag
    // is cut.
    assert_eq!(
        created_tag_names(&writer),
        vec!["rust/core@0.2.0".to_string()]
    );
}

#[test]
fn umbrella_tag_mode_creates_only_the_umbrella_tag() {
    let plan = umbrella_and_per_module(toven_ports::TagMode::Umbrella);
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");
    apply_umbrella_train(&plan, &writer);
    assert_eq!(created_tag_names(&writer), vec!["v0.2.0".to_string()]);
}

#[test]
fn both_tag_mode_creates_per_module_and_umbrella_tags() {
    let plan = umbrella_and_per_module(toven_ports::TagMode::Both);
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");
    apply_umbrella_train(&plan, &writer);
    let mut created = created_tag_names(&writer);
    created.sort();
    assert_eq!(
        created,
        vec!["rust/core@0.2.0".to_string(), "v0.2.0".to_string()]
    );
}

#[test]
fn two_modules_sharing_a_tag_collapse_into_a_single_release_train() {
    let mut core = entry("core", Version::new(0, 2, 0), true, 0);
    core.tag_format = Some("v{version}".into());
    // Plan-time tag resolution renders both modules to the same tag: a
    // single-version workspace collapses onto one shared repository tag.
    core.planned_tag = Some("v0.2.0".into());
    let mut app = entry("app", Version::new(0, 2, 0), true, 1);
    app.tag_format = Some("v{version}".into());
    app.planned_tag = Some("v0.2.0".into());
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core, app]);
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

    let stats = release_apply(
        &plan,
        &[module("core"), module("app")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect("modules sharing a tag collapse into one release train");

    // The shared tag is created exactly once for the whole train.
    assert_eq!(stats.tagged_modules, 1);
    let create_tags: Vec<_> = writer
        .writes()
        .into_iter()
        .filter_map(|w| match w {
            VcsWrite::CreateTag { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(create_tags, vec!["v0.2.0".to_string()]);

    // The commit message lists the collapsed tag once, and the push carries
    // a single tag refspec.
    let recorded = writer.writes();
    assert_eq!(
        recorded[0],
        VcsWrite::Commit {
            message: "release: v0.2.0".into(),
            paths: vec![
                "crates/core/Cargo.toml".into(),
                "crates/app/Cargo.toml".into(),
            ],
        }
    );
    let tag_refspecs: Vec<_> = recorded
        .iter()
        .filter_map(|w| match w {
            VcsWrite::Push { refspecs, .. } => Some(refspecs.clone()),
            _ => None,
        })
        .flatten()
        .filter(|r| r.starts_with("refs/tags/"))
        .collect();
    assert_eq!(tag_refspecs, vec!["refs/tags/v0.2.0".to_string()]);
}

#[test]
fn modules_sharing_a_tag_with_divergent_annotations_are_rejected() {
    let mut core = entry("core", Version::new(0, 2, 0), true, 0);
    core.tag_format = Some("v{version}".into());
    core.planned_tag = Some("v0.2.0".into());
    core.tag_message = Some("core annotation".into());
    let mut app = entry("app", Version::new(0, 2, 0), true, 1);
    app.tag_format = Some("v{version}".into());
    app.planned_tag = Some("v0.2.0".into());
    app.tag_message = Some("app annotation".into());
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core, app]);
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core"), module("app")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a shared tag with conflicting annotations must fail closed");

    let message = error.to_string();
    assert!(message.contains("v0.2.0"), "{message}");
    assert!(message.contains("annotation"), "{message}");
    assert!(writer.writes().is_empty());
}

#[test]
fn a_module_without_a_release_target_is_rejected_before_any_mutation() {
    // The go module has no registered target; the failure must surface
    // before the rust module's mutation, not inside `prepare`.
    let go_ref = ModuleRef::new(EcosystemId::new("go").unwrap(), "cache-redis").unwrap();
    let mut go_module = Module::new(go_ref.clone(), RepoPath::new("cache/redis").unwrap());
    go_module.manifest = Some(RepoPath::new("cache/redis/go.mod").unwrap());
    let mut go_entry = entry("core", Version::new(2, 0, 0), true, 1);
    go_entry.module = ModuleKey::bare(go_ref);
    go_entry.planned_tag = Some("cache/redis/v2.0.0".into());

    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0), go_entry],
    );
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core"), go_module],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a missing release target must fail closed before mutation");

    assert!(error.to_string().contains("has no release target"));
    assert!(writer.writes().is_empty(), "no VCS write may happen");
    assert!(
        target.calls().is_empty(),
        "no target mutation/package may happen: {:?}",
        target.calls()
    );
}

#[test]
fn a_publish_failure_after_the_commit_carries_forward_only_recovery_guidance() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 2, 0), true, 0)],
    );
    let target = FakeReleaseTarget::new().with_publish_failure("registry unavailable");
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target)]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a post-commit publish failure must surface recovery guidance");

    let message = error.to_string();
    assert!(message.contains("publication"), "{message}");
    assert!(message.contains("registry unavailable"), "{message}");
    assert!(message.contains("toven release status"), "{message}");
    assert!(message.contains("forward fix"), "{message}");
    // The commit is past the rollback boundary: it happened, and no
    // worktree restore may be attempted for a post-commit failure.
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Commit { .. }))
    );
    assert!(
        !writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree))
    );
}

#[test]
fn a_push_failure_after_the_commit_carries_forward_only_recovery_guidance() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 2, 0), true, 0)],
    );
    let writer = FakeVcsWriter::new().with_push_failure("remote rejected");

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            no_push: false,
            ..Default::default()
        },
    )
    .expect_err("a post-commit push failure must surface recovery guidance");

    let message = error.to_string();
    assert!(message.contains("push"), "{message}");
    assert!(message.contains("remote rejected"), "{message}");
    assert!(message.contains("toven release status"), "{message}");
    assert!(message.contains("forward fix"), "{message}");
    // The commit and tag are past the rollback boundary: no worktree
    // restore may be attempted for a post-commit push failure.
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Commit { .. }))
    );
    assert!(
        !writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree))
    );
}

#[test]
fn an_all_tags_exist_release_resumes_without_git_mutation() {
    // The planned tag already exists on the remote: the commit, tag, and
    // push happened on a prior attempt, so APPLY resumes — no manifest
    // mutation, commit, tag, or push — and the version is already published,
    // so the publish loop is a clean no-op.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 2, 0), false, 0)],
    );
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new();
    let reader =
        FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &reader,
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("an already-tagged release resumes rather than failing closed");

    assert!(stats.resumed, "the run is marked resumed");
    assert_eq!(stats.tagged_modules, 0);
    assert!(
        writer.writes().is_empty(),
        "no commit/tag/push may happen on resume: {:?}",
        writer.writes()
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
fn a_resume_publishes_a_version_the_registry_still_lacks() {
    // The planned tag exists (commit, tag, and push happened on a prior
    // attempt) but the registry publish never completed: the entry is still
    // publish-needed. A resume must package it (no manifest mutation) and
    // publish it, completing the interrupted publish rather than failing on a
    // missing artifact.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 2, 0), true, 0)],
    );
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new();
    let reader =
        FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &reader,
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("a resume completes the interrupted publish");

    assert!(stats.resumed, "the run is marked resumed");
    assert_eq!(stats.packaged_artifacts, 1);
    assert_eq!(stats.published_modules, 1);
    assert_eq!(stats.tagged_modules, 0);
    assert!(
        writer.writes().is_empty(),
        "no commit/tag/push may happen on resume: {:?}",
        writer.writes()
    );
    assert!(
        !target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::ApplyRelease { .. })),
        "a resume never mutates a manifest: {:?}",
        target.calls()
    );
    assert!(
        target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::Package(_)))
            && target
                .calls()
                .iter()
                .any(|call| matches!(call, ReleaseCall::Publish(_))),
        "a resume packages and publishes the missing version: {:?}",
        target.calls()
    );
}

#[test]
fn a_partial_planned_tag_set_is_rejected_before_any_mutation() {
    // One of two planned tags exists: an interrupted or divergent release,
    // never a resume — it fails closed with immutable/forward-fix guidance
    // before any mutation.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", Version::new(0, 2, 0), true, 0),
            entry("app", Version::new(0, 2, 0), true, 1),
        ],
    );
    let writer = FakeVcsWriter::new();
    let reader =
        FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

    let error = release_apply(
        &plan,
        &[module("core"), module("app")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &reader,
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a partial tag overlap must fail closed before mutation");

    let message = error.to_string();
    assert!(message.contains("rust/core@0.2.0"), "{message}");
    assert!(message.contains("rust/app@0.2.0"), "{message}");
    assert!(message.contains("immutable"), "{message}");
    assert!(message.contains("forward-fix"), "{message}");
    assert!(writer.writes().is_empty(), "no VCS write may happen");
}

#[test]
fn a_maintainer_owned_apply_publishes_against_the_existing_tag_without_mutating() {
    // The maintainer already created the tag `rust/core@0.2.0` (and the
    // hosted Release) in the forge, pointing at the checked-out HEAD. A
    // maintainer-owned apply verifies that tag, then publishes the version
    // the registry still lacks against it — mutating no manifest and
    // creating no commit, tag, or push.
    let mut owned = entry("core", Version::new(0, 2, 0), true, 0);
    owned.entrypoint = toven_model::Entrypoint::Maintainer;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![owned]);
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new();
    let reader = FakeVcsReader::new()
        .with_rev_parse("deadbee")
        .with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &reader,
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("a maintainer-owned apply publishes against the existing tag");

    assert_eq!(stats.published_modules, 1);
    assert_eq!(stats.tagged_modules, 0);
    assert!(
        writer.writes().is_empty(),
        "no commit/tag/push may happen in maintainer-owned apply: {:?}",
        writer.writes()
    );
    assert!(
        !target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::ApplyRelease { .. })),
        "a maintainer-owned apply never mutates a manifest: {:?}",
        target.calls()
    );
    assert!(
        target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::Publish(_))),
        "a maintainer-owned apply publishes against the existing tag: {:?}",
        target.calls()
    );
}

#[test]
fn maintainer_umbrella_mode_verifies_only_the_umbrella_tag() {
    // In umbrella tag mode the maintainer cuts only the umbrella tag
    // (`v0.2.0`); the per-module `core` tag is never created, so
    // verification must not require it — the absent per-module tag is not a
    // fail-closed condition.
    let mut suite = entry("suite", Version::new(0, 2, 0), true, 0);
    suite.entrypoint = toven_model::Entrypoint::Maintainer;
    suite.tag_format = Some("v{version}".into());
    suite.planned_tag = Some("v0.2.0".into());
    suite.umbrella = true;
    suite.tag_mode = Some(toven_ports::TagMode::Umbrella);
    let mut core = entry("core", Version::new(0, 2, 0), true, 1);
    core.entrypoint = toven_model::Entrypoint::Maintainer;
    core.tag_mode = Some(toven_ports::TagMode::Umbrella);
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![suite, core]);
    let writer = FakeVcsWriter::new();
    // Only the umbrella tag exists at HEAD; no per-module `rust/core@…` tag.
    let reader = FakeVcsReader::new()
        .with_rev_parse("deadbee")
        .with_tags(vec![TagRef::new("v0.2.0", Oid::new("deadbee"))]);

    release_apply(
        &plan,
        &[module("suite"), module("core")],
        &targets(vec![("suite", FakeReleaseTarget::new())]),
        &reader,
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("umbrella-mode maintainer verify requires only the umbrella tag");

    assert!(writer.writes().is_empty(), "{:?}", writer.writes());
}

#[test]
fn a_maintainer_owned_apply_fails_closed_when_the_release_tag_is_absent() {
    // No tag exists for the planned version: the maintainer has not cut the
    // Release yet, or the manifest version and the created tag diverge. Toven
    // refuses to proceed rather than creating or moving the tag itself.
    let mut owned = entry("core", Version::new(0, 2, 0), true, 0);
    owned.entrypoint = toven_model::Entrypoint::Maintainer;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![owned]);
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("an absent maintainer-owned tag fails closed");

    let message = error.to_string();
    assert!(message.contains("rust/core@0.2.0"), "{message}");
    assert!(message.contains("never creates or moves"), "{message}");
    assert!(
        writer.writes().is_empty(),
        "no VCS write may happen when a maintainer-owned tag is absent"
    );
}

#[test]
fn a_maintainer_owned_apply_fails_closed_when_the_tag_diverges_from_head() {
    // The tag exists but points at an earlier commit than the checked-out
    // HEAD (e.g. CI checked out a branch tip past the maintainer's tag). A
    // maintainer-owned apply packages and publishes from HEAD, so attaching
    // artifacts to a tag that names a different commit would produce a
    // divergent Release — Toven fails closed and touches nothing.
    let mut owned = entry("core", Version::new(0, 2, 0), true, 0);
    owned.entrypoint = toven_model::Entrypoint::Maintainer;
    let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![owned]);
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new();
    let reader = FakeVcsReader::new()
        .with_rev_parse("cafef00d")
        .with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &reader,
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("a maintainer-owned tag that diverges from HEAD fails closed");

    let message = error.to_string();
    assert!(message.contains("rust/core@0.2.0"), "{message}");
    assert!(message.contains("checked-out HEAD"), "{message}");
    assert!(message.contains("cafef00d"), "{message}");
    assert!(
        writer.writes().is_empty(),
        "no VCS write may happen when a maintainer-owned tag diverges from HEAD"
    );
    assert!(
        !target.calls().iter().any(|call| matches!(
            call,
            ReleaseCall::Publish(_) | ReleaseCall::ApplyRelease { .. }
        )),
        "no publish or mutation may happen when the tag diverges: {:?}",
        target.calls()
    );
}

#[test]
fn configured_templates_render_commit_and_lightweight_tag() {
    let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
    entry.commit_message = Some("release".into());
    let lightweight = entry.clone();
    let mut annotated = entry;
    annotated.module = mkey("app");
    annotated.planned_tag = Some("rust/app@1.2.3".into());
    annotated.tag_message = Some("tag {ecosystem}/{module} {version}".into());
    let writer = FakeVcsWriter::new();

    release_apply(
        &ReleasePlan::new(BumpPolicy::SemverCascade, vec![lightweight, annotated]),
        &[module("core"), module("app")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("template release");

    let recorded = writer.writes();
    assert!(matches!(
        &recorded[0],
        VcsWrite::Commit { message, .. } if message == "release"
    ));
    assert!(matches!(
        &recorded[1],
        VcsWrite::CreateTag { message: None, .. }
    ));
    assert!(matches!(
        &recorded[2],
        VcsWrite::CreateTag { message: Some(message), .. }
            if message == "tag rust/app 1.2.3"
    ));
}

#[test]
fn signed_tag_flag_flows_through_to_the_writer() {
    let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
    entry.planned_tag = Some("rust/core@1.2.3".into());
    entry.tag_message = Some("release {version}".into());
    entry.signer = Some(toven_ports::TagSigner {
        format: Some(toven_ports::SignFormat::Ssh),
        key: Some("KEYID".into()),
    });
    let writer = FakeVcsWriter::new();

    release_apply(
        &ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]),
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect("signed tag release");

    assert!(
        writer.writes().iter().any(|write| matches!(
            write,
            VcsWrite::CreateTag { signer: Some(signer), message: Some(message), .. }
                if message == "release 1.2.3"
                    && signer.format == Some(toven_ports::SignFormat::Ssh)
                    && signer.key.as_deref() == Some("KEYID")
        )),
        "the signed, annotated tag must reach the writer: {:?}",
        writer.writes()
    );
}

#[test]
fn signed_tag_preflight_fails_before_any_mutation() {
    let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
    entry.tag_message = Some("release {version}".into());
    entry.signer = Some(toven_ports::TagSigner::default());
    let writer =
        FakeVcsWriter::new().with_tag_signer_preflight_failure("user.signingkey is not configured");
    let target = FakeReleaseTarget::new();

    let error = release_apply(
        &ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]),
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("missing signing key fails before mutation");

    assert!(error.to_string().contains("user.signingkey"), "{error}");
    assert!(
        writer.writes().is_empty(),
        "no VCS write may happen after signer preflight failure: {:?}",
        writer.writes()
    );
    assert!(
        !target
            .calls()
            .iter()
            .any(|call| matches!(call, ReleaseCall::ApplyRelease { .. })),
        "manifest mutation must not run after signer preflight failure: {:?}",
        target.calls()
    );
}

#[test]
fn shared_tag_requires_consistent_signer_settings() {
    let mut signed = entry("core", Version::new(1, 2, 3), true, 0);
    signed.planned_tag = Some("v1.2.3".into());
    signed.tag_message = Some("release {version}".into());
    signed.signer = Some(toven_ports::TagSigner {
        format: Some(toven_ports::SignFormat::Ssh),
        key: Some("KEYID".into()),
    });
    let mut unsigned = entry("app", Version::new(1, 2, 3), true, 1);
    unsigned.planned_tag = Some("v1.2.3".into());
    unsigned.tag_message = Some("release {version}".into());
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &ReleasePlan::new(BumpPolicy::SemverCascade, vec![signed, unsigned]),
        &[module("core"), module("app")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("one shared tag cannot mix signing settings");

    let message = error.to_string();
    assert!(message.contains("v1.2.3"), "{message}");
    assert!(message.contains("signing settings"), "{message}");
    assert!(writer.writes().is_empty(), "no VCS write may happen");
}

#[test]
fn invalid_commit_template_does_not_mutate_or_restore() {
    let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
    entry.commit_message = Some("{invalid}".into());
    let writer = FakeVcsWriter::new();
    let target = FakeReleaseTarget::new();

    let error = release_apply(
        &ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]),
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("invalid commit template");

    assert!(error.to_string().contains("release.commit_message"));
    assert!(writer.writes().is_empty());
    assert!(target.calls().is_empty());
}

#[test]
fn repository_scoped_settings_must_agree_between_modules() {
    let first = entry("core", Version::new(1, 0, 0), true, 0);
    let cases = [
        ("push", {
            let mut second = entry("app", Version::new(1, 0, 0), true, 1);
            second.push = PushPolicy::Disabled;
            second
        }),
        ("remote", {
            let mut second = entry("app", Version::new(1, 0, 0), true, 1);
            second.remote = "release".into();
            second
        }),
        ("branches", {
            let mut second = entry("app", Version::new(1, 0, 0), true, 1);
            second.branches = vec!["release".into()];
            second
        }),
        ("commit_message", {
            let mut second = entry("app", Version::new(1, 0, 0), true, 1);
            second.commit_message = Some("release".into());
            second
        }),
    ];
    for (field, second) in cases {
        let error = reconcile_repo_settings(&[first.clone(), second])
            .expect_err("conflicting repository setting");
        assert!(
            error.to_string().contains(&format!("release.{field}")),
            "{error}"
        );
    }
}

#[test]
fn tag_only_run_commits_and_tags_without_publishing() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new();
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            publish: false,
            ..Default::default()
        },
    )
    .expect("tag-only release apply");

    assert_eq!(stats.tagged_modules, 1);
    assert_eq!(stats.published_modules, 0);
    assert!(
        writer
            .writes()
            .iter()
            .any(|w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "rust/core@0.1.1"))
    );
    assert!(
        !target
            .calls()
            .iter()
            .any(|c| matches!(c, ReleaseCall::Publish(_))),
        "tag-only run must not publish"
    );
}

#[test]
fn mixed_ecosystem_umbrella_tags_each_member_with_its_own_scheme() {
    // A Rust crate (crates.io tag grammar) and a Go module (path-based git tag
    // grammar) release over the one topological order, each carrying its own
    // target-owned tag scheme.
    let go_ref = ModuleRef::new(EcosystemId::new("go").unwrap(), "cache-redis").unwrap();
    let go_key = ModuleKey::bare(go_ref.clone());
    let mut go_module = Module::new(go_ref, RepoPath::new("cache/redis").unwrap());
    go_module.manifest = Some(RepoPath::new("cache/redis/go.mod").unwrap());
    let mut go_entry = entry("core", Version::new(2, 0, 0), true, 1);
    go_entry.module = go_key;
    // Plan-time tag resolution uses the go target's path-based scheme.
    go_entry.planned_tag = Some("cache/redis/v2.0.0".into());

    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0), go_entry],
    );

    let go_target = FakeReleaseTarget::new().with_tag_scheme(TagScheme::new("cache/redis/v", ""));
    let mut map = crate::ReleaseTargets::new();
    map.insert(
        (None, EcosystemId::new("rust").unwrap()),
        Box::new(FakeReleaseTarget::new()),
    );
    map.insert((None, EcosystemId::new("go").unwrap()), Box::new(go_target));
    let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

    release_apply(
        &plan,
        &[module("core"), go_module],
        &map,
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions {
            publish: false,
            ..Default::default()
        },
    )
    .expect("mixed-ecosystem release apply");

    let recorded = writer.writes();
    assert_eq!(
        recorded[0],
        VcsWrite::Commit {
            message: "release: rust/core@0.1.1, cache/redis/v2.0.0".into(),
            paths: vec!["crates/core/Cargo.toml".into(), "cache/redis/go.mod".into(),],
        }
    );
    assert!(
        recorded
            .iter()
            .any(|w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "rust/core@0.1.1")),
        "rust member keeps its crates.io tag grammar"
    );
    assert!(
        recorded
            .iter()
            .any(|w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "cache/redis/v2.0.0")),
        "go member uses its path-based git tag grammar"
    );
}

#[test]
fn restores_worktree_when_commit_fails() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let writer = FakeVcsWriter::new().with_commit_failure("commit failed");

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("commit failure must surface");

    assert!(error.to_string().contains("commit failed"));
    assert_eq!(
        writer.writes(),
        vec![
            VcsWrite::Commit {
                message: "release: rust/core@0.1.1".into(),
                paths: vec!["crates/core/Cargo.toml".into()],
            },
            VcsWrite::RestoreWorktree
        ]
    );
}

#[test]
fn prepare_failure_reports_restore_failure_without_losing_original_error() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let writer = FakeVcsWriter::new().with_restore_failure("restore failed");

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![(
            "core",
            FakeReleaseTarget::new().with_package_failure("package failed"),
        )]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("prepare and restore failures must surface together");

    let message = error.to_string();
    assert!(message.contains("release prepare failed"));
    assert!(message.contains("package failed"));
    assert!(message.contains("restore failed"));
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(
        error
            .cause()
            .is_some_and(|cause| cause.to_string().contains("package failed"))
    );
    assert_eq!(writer.writes(), vec![VcsWrite::RestoreWorktree]);
}

#[test]
fn commit_failure_reports_restore_failure_without_losing_original_error() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let writer = FakeVcsWriter::new()
        .with_commit_failure("commit failed")
        .with_restore_failure("restore failed");

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("commit and restore failures must surface together");

    let message = error.to_string();
    assert!(message.contains("release commit failed"));
    assert!(message.contains("commit failed"));
    assert!(message.contains("restore failed"));
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(
        error
            .cause()
            .is_some_and(|cause| cause.to_string().contains("commit failed"))
    );
    assert_eq!(
        writer.writes(),
        vec![
            VcsWrite::Commit {
                message: "release: rust/core@0.1.1".into(),
                paths: vec!["crates/core/Cargo.toml".into()],
            },
            VcsWrite::RestoreWorktree
        ]
    );
}

#[test]
fn dirty_worktree_is_rejected() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &dirty(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("dirty worktree must be rejected");
    assert!(error.to_string().contains("uncommitted change"));
    // The guard names the offending path (not just a count) so a CI-only dirty
    // file — e.g. a regenerated `go.sum` — is diagnosable from the error alone.
    assert!(
        error.to_string().contains("modified go.sum"),
        "clean-tree error should name the dirty path: {error}"
    );
    assert!(
        writer.writes().is_empty(),
        "no writes on a tripped guardrail"
    );
}

#[test]
fn dirty_worktree_error_bounds_the_named_paths() {
    // A pathologically dirty tree must not produce an unbounded message: the
    // guard names up to 20 paths and summarizes the rest as `… and N more`.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let changes: Vec<ChangeRecord> = (0..25)
        .map(|i| ChangeRecord::new(format!("mod{i:02}/go.sum"), ChangeStatus::Modified))
        .collect();
    let reader = FakeVcsReader::new().with_worktree_status(changes);

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", FakeReleaseTarget::new())]),
        &reader,
        &FakeVcsWriter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect_err("dirty worktree must be rejected");
    let message = error.to_string();
    assert!(message.contains("25 uncommitted change(s)"), "{message}");
    assert!(message.contains("… and 5 more"), "{message}");
}

#[test]
fn no_option_bypasses_the_clean_tree_guardrail() {
    // The clean-tree guardrail has no bypass: a dirty tree is always rejected,
    // regardless of options. This regression-tests the removal of `--allow-dirty`.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    for options in [
        ReleaseApplyOptions::default(),
        ReleaseApplyOptions {
            no_push: false,
            publish: true,
            ..ReleaseApplyOptions::default()
        },
    ] {
        let writer = FakeVcsWriter::new();
        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &dirty(),
            &writer,
            &options,
        )
        .expect_err("dirty worktree must always be rejected");
        assert!(error.to_string().contains("uncommitted change"));
    }
}

#[test]
fn package_failure_rolls_back_before_commit() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let writer = FakeVcsWriter::new();

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![(
            "core",
            FakeReleaseTarget::new().with_package_failure("boom"),
        )]),
        &FakeVcsReader::new(),
        &writer,
        &ReleaseApplyOptions::default(),
    )
    .expect_err("package failure surfaces");
    assert!(error.to_string().contains("boom"));

    let recorded = writer.writes();
    assert_eq!(recorded, vec![VcsWrite::RestoreWorktree]);
    assert!(
        !recorded
            .iter()
            .any(|w| matches!(w, VcsWrite::Commit { .. }))
    );
}

#[test]
fn already_published_version_is_pre_skipped() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect("pre-skip");

    assert_eq!(stats.published_modules, 0);
    assert_eq!(stats.skipped_published_modules, 1);
    assert!(
        !target
            .calls()
            .iter()
            .any(|c| matches!(c, ReleaseCall::Publish(_))),
        "an already-published version must not be re-published"
    );
}

#[test]
fn already_published_outcome_is_resume_safe() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new().with_publish_outcome(PublishOutcome::AlreadyPublished);

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target)]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect("resume-safe already-published");

    assert_eq!(stats.published_modules, 0);
    assert_eq!(stats.skipped_published_modules, 1);
}

#[test]
fn rate_limited_publish_retries_within_budget() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new().with_publish_outcomes(vec![
        PublishOutcome::RateLimited { retry_after: None },
        PublishOutcome::Published,
    ]);

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target)]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect("retry then publish");

    assert_eq!(stats.published_modules, 1);
    assert_eq!(stats.rate_limited_waits, 1);
}

#[test]
fn rate_limited_publish_surfaces_exhausted_budget() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), true, 0)],
    );
    let target = FakeReleaseTarget::new()
        .with_publish_outcome(PublishOutcome::RateLimited { retry_after: None });

    let error = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target)]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new(),
        &ReleaseApplyOptions {
            retry_budget: 2,
            ..Default::default()
        },
    )
    .expect_err("exhausted budget surfaces");
    assert!(error.to_string().contains("rate-limit retry budget"));
}

#[test]
fn entries_without_publish_needed_are_not_published() {
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(0, 1, 1), false, 0)],
    );
    // A tag-only module is never packaged: `cargo package` on an unpublished
    // workspace crate cannot resolve its intra-workspace deps from the
    // registry and exits non-zero. A package attempt here would fail the
    // whole tag-only release, so wire the double to blow up if it happens.
    let target = FakeReleaseTarget::new().with_package_failure("tag-only must not package");

    let stats = release_apply(
        &plan,
        &[module("core")],
        &targets(vec![("core", target.clone())]),
        &FakeVcsReader::new(),
        &FakeVcsWriter::new(),
        &ReleaseApplyOptions::default(),
    )
    .expect("apply without publish");

    assert_eq!(stats.mutated_modules, 1);
    assert_eq!(stats.packaged_artifacts, 0);
    assert_eq!(stats.tagged_modules, 1);
    assert_eq!(stats.published_modules, 0);
    assert!(
        !target
            .calls()
            .iter()
            .any(|c| matches!(c, ReleaseCall::Package(_) | ReleaseCall::Publish(_)))
    );
}

#[test]
fn standalone_push_lands_named_branch_and_tags_on_a_real_bare_remote() {
    use rskit_git::RefManager;
    use toven_ports::VcsWriter;
    use toven_testkit::TestWorkspace;
    use toven_testkit::git::{GitScenario, ref_map_at};
    use toven_vcs::RskitGitVcs;

    let workspace = TestWorkspace::new("release-standalone-real-push");
    let work = workspace.child("work").expect("work dir");
    let bare = workspace.child("remote.git").expect("bare dir");

    // A real working repo, committed on a named, non-`main` branch.
    let scenario = GitScenario::init(&work).expect("init work");
    scenario
        .commit_file("Cargo.toml", "name=core\n", "import")
        .expect("initial commit");
    scenario
        .branch_and_checkout("release-train")
        .expect("named branch");
    GitScenario::init_bare(&bare).expect("init bare remote");
    scenario.add_remote("origin", &bare).expect("wire remote");

    // Lightweight release tag the plan pushes (target oid == commit oid, so
    // local and remote ref-maps compare directly with no peel ambiguity).
    scenario
        .repo()
        .create_tag("rust/core@1.0.0", "HEAD", None)
        .expect("release tag");
    let local = scenario.ref_map().expect("local refs");

    // The exact refspecs the standalone push uses for this branch.
    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![entry("core", Version::new(1, 0, 0), true, 0)],
    );
    let refspecs = super::push_refspecs(&plan, Some("release-train")).expect("refspecs");
    assert_eq!(
        refspecs,
        vec![
            "refs/heads/release-train".to_string(),
            "refs/tags/rust/core@1.0.0".to_string(),
        ]
    );

    // Push through the real rskit-git-backed writer to the real bare remote.
    RskitGitVcs::open(&work)
        .expect("open work")
        .push("origin", &refspecs)
        .expect("push to bare remote");

    // The bare remote received exactly the named branch and the tag, at the
    // same oids as the local repo — the `HEAD` refspec never created these.
    let remote = ref_map_at(&bare).expect("remote refs");
    assert_eq!(
        remote.get("refs/heads/release-train"),
        local.get("refs/heads/release-train"),
    );
    assert_eq!(
        remote.get("refs/tags/rust/core@1.0.0"),
        local.get("refs/tags/rust/core@1.0.0"),
    );
    assert!(!remote.contains_key("refs/heads/HEAD"));
    assert!(!remote.contains_key("refs/heads/main"));
}

#[test]
fn federated_style_multi_module_push_lands_every_tag_on_a_real_bare_remote() {
    // The federated member push (release::federated::commit_member_shard)
    // shares `push_refspecs` and the same rskit-git writer as the standalone
    // path, adding only `reader().current_branch()`. This proves that shared
    // mechanism pushes the resolved branch plus every module tag to a real
    // custom-named remote for a multi-module member shard.
    use rskit_git::RefManager;
    use toven_ports::{VcsReader, VcsWriter};
    use toven_testkit::TestWorkspace;
    use toven_testkit::git::{GitScenario, ref_map_at};
    use toven_vcs::RskitGitVcs;

    let workspace = TestWorkspace::new("release-federated-real-push");
    let work = workspace.child("work").expect("work dir");
    let bare = workspace.child("upstream.git").expect("bare dir");

    let scenario = GitScenario::init(&work).expect("init work");
    scenario
        .commit_file("Cargo.toml", "name=member\n", "import")
        .expect("initial commit");
    scenario
        .branch_and_checkout("member-release")
        .expect("named branch");
    GitScenario::init_bare(&bare).expect("init bare remote");
    scenario.add_remote("upstream", &bare).expect("wire remote");

    for tag in ["rust/core@1.0.0", "rust/app@1.0.0"] {
        scenario
            .repo()
            .create_tag(tag, "HEAD", None)
            .expect("release tag");
    }
    let local = scenario.ref_map().expect("local refs");

    let plan = ReleasePlan::new(
        BumpPolicy::SemverCascade,
        vec![
            entry("core", Version::new(1, 0, 0), true, 0),
            entry("app", Version::new(1, 0, 0), true, 1),
        ],
    );

    // Resolve the branch exactly as the federated push does.
    let reader = RskitGitVcs::open(&work).expect("open reader");
    let branch = reader.current_branch().expect("current branch");
    assert_eq!(branch, "member-release");

    let refspecs = super::push_refspecs(&plan, Some(&branch)).expect("refspecs");
    assert_eq!(
        refspecs,
        vec![
            "refs/heads/member-release".to_string(),
            "refs/tags/rust/core@1.0.0".to_string(),
            "refs/tags/rust/app@1.0.0".to_string(),
        ]
    );

    RskitGitVcs::open(&work)
        .expect("open writer")
        .push("upstream", &refspecs)
        .expect("push to bare remote");

    let remote = ref_map_at(&bare).expect("remote refs");
    for refname in [
        "refs/heads/member-release",
        "refs/tags/rust/core@1.0.0",
        "refs/tags/rust/app@1.0.0",
    ] {
        assert_eq!(remote.get(refname), local.get(refname), "{refname}");
    }
    assert!(!remote.contains_key("refs/heads/HEAD"));
}
