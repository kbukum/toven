//! Behavioral tests for the crates.io release target's version I/O and manifest
//! mutation. The target resolves a module's repo-relative manifest against the
//! process working directory, so these tests pin the working directory to a
//! materialized sample repo via a `CurrentDirGuard` — which serializes the
//! process-wide change and restores the previous directory on drop.

use std::sync::Arc;

use rskit_version::semver::Version;
use toven_exec::ProcessToolRunner;
use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{ManifestMutator, Packager, Publisher, ReleaseMutation, VersionSource};
use toven_rust::CargoRegistryTarget;
use toven_testkit::{CurrentDirGuard, SampleRepo};

fn target() -> CargoRegistryTarget {
    CargoRegistryTarget::new(Arc::new(ProcessToolRunner::new()))
}

fn app_module() -> Module {
    let id = ModuleRef::new(EcosystemId::new("rust").unwrap(), "app").unwrap();
    let mut module = Module::new(id, RepoPath::new("crates/app").unwrap());
    module.package = Some("app".to_string());
    module.manifest = Some(RepoPath::new("crates/app/Cargo.toml").unwrap());
    module
}

fn member_module(name: &str) -> Module {
    let id = ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap();
    let mut module = Module::new(id, RepoPath::new(format!("crates/{name}")).unwrap());
    module.package = Some(name.to_string());
    module.manifest = Some(RepoPath::new(format!("crates/{name}/Cargo.toml")).unwrap());
    module
}

#[test]
fn apply_release_bumps_the_workspace_root_not_the_member() {
    // A single-version workspace: the member inherits `version.workspace = true`.
    // The bump must land on the root's `[workspace.package].version` alone and
    // must NOT stamp a literal `version` into the member's `[package]`.
    let repo = SampleRepo::materialize("rust/workspace-inherited").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = target();
    let module = app_module();
    let changed = target
        .apply_release(&module, &ReleaseMutation::version(Version::new(0, 4, 0)))
        .expect("apply release");

    // Only the workspace-root manifest is reported as rewritten.
    assert_eq!(changed, vec![RepoPath::new("Cargo.toml").unwrap()]);

    let member = std::fs::read_to_string(repo.child("crates/app/Cargo.toml")).expect("read member");
    assert!(
        member.contains("version.workspace = true"),
        "member inheritance preserved: {member}"
    );
    assert!(
        !member.contains("version = \"0.4.0\""),
        "member must not gain a literal version: {member}"
    );

    let root = std::fs::read_to_string(repo.child("Cargo.toml")).expect("read root");
    assert!(
        root.contains("[workspace.package]") && root.contains("version = \"0.4.0\""),
        "root workspace version bumped: {root}"
    );

    // Reader and writer agree on the same source of truth.
    let version = target.declared_version(&module).expect("re-read version");
    assert_eq!(version, Some(Version::new(0, 4, 0)));
}

#[test]
fn apply_release_on_shared_root_routes_every_member_to_one_root() {
    // Many members of a single-version workspace resolve to one workspace root.
    // The FIRST member to request the bump actually rewrites the shared root and
    // reports it; every later member requesting the SAME version finds the root
    // already at target, so its write is a no-op that reports NO path. The root
    // is thus written exactly once, and the empty second result proves we no
    // longer restage an untouched manifest per member.
    let repo = SampleRepo::materialize("rust/workspace-inherited-multi").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = target();

    let first = target
        .apply_release(
            &member_module("app"),
            &ReleaseMutation::version(Version::new(0, 4, 0)),
        )
        .expect("apply release for first member");
    assert_eq!(
        first,
        vec![RepoPath::new("Cargo.toml").unwrap()],
        "the first member rewrites and reports the one workspace root"
    );

    let second = target
        .apply_release(
            &member_module("lib"),
            &ReleaseMutation::version(Version::new(0, 4, 0)),
        )
        .expect("apply release for second member");
    assert!(
        second.is_empty(),
        "the second member finds the root already at target and reports no write: {second:?}"
    );

    for name in ["app", "lib"] {
        let member =
            std::fs::read_to_string(repo.child(format!("crates/{name}/Cargo.toml"))).expect("read");
        assert!(
            member.contains("version.workspace = true") && !member.contains("version = \"0.4.0\""),
            "member '{name}' keeps its inherited version: {member}"
        );
    }

    let root = std::fs::read_to_string(repo.child("Cargo.toml")).expect("read root");
    assert!(
        root.contains("version = \"0.4.0\""),
        "shared workspace version bumped once: {root}"
    );
}

#[test]
fn apply_release_rejects_divergent_bumps_to_a_shared_workspace_root() {
    // Two members of one single-version workspace requesting DIFFERENT versions
    // is a real conflict: their inherited `[workspace.package].version` is shared,
    // so a silent last-writer-wins would tag one member at a version the root no
    // longer carries. The second, divergent request must fail closed with a typed
    // `release.version` error rather than corrupt the release.
    let repo = SampleRepo::materialize("rust/workspace-inherited-multi").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = target();

    target
        .apply_release(
            &member_module("app"),
            &ReleaseMutation::version(Version::new(0, 4, 0)),
        )
        .expect("first member bumps the shared root");

    let mut lib_mutation = ReleaseMutation::version(Version::new(0, 5, 0));
    let dep_ref = ModuleRef::new(EcosystemId::new("rust").unwrap(), "dep").unwrap();
    lib_mutation.dep_floor_updates.insert(dep_ref, Version::new(1, 0, 0));

    let lib_text_before =
        std::fs::read_to_string(repo.child("crates/lib/Cargo.toml")).expect("read lib before");

    let error = target
        .apply_release(&member_module("lib"), &lib_mutation)
        .expect_err("a divergent sibling bump to the same root must fail closed");
    let message = error.to_string();
    assert!(
        message.contains("divergent"),
        "expected a typed divergent-bump conflict, got: {message}"
    );

    let lib_text_after =
        std::fs::read_to_string(repo.child("crates/lib/Cargo.toml")).expect("read lib after");
    assert_eq!(
        lib_text_before, lib_text_after,
        "divergent bump failure must leave no partial manifest writes on disk"
    );
}

#[test]
fn reads_the_declared_version_from_the_manifest() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let version = target()
        .declared_version(&app_module())
        .expect("declared version");
    assert_eq!(version, Some(Version::new(0, 1, 0)));
}

#[test]
fn apply_release_rewrites_the_declared_version() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = target();
    let module = app_module();
    target
        .apply_release(&module, &ReleaseMutation::version(Version::new(0, 2, 0)))
        .expect("apply release");

    let version = target.declared_version(&module).expect("re-read version");
    assert_eq!(version, Some(Version::new(0, 2, 0)));
}

#[test]
fn reads_a_version_inherited_from_the_workspace_root() {
    let repo = SampleRepo::materialize("rust/workspace-inherited").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let version = target()
        .declared_version(&app_module())
        .expect("inherited version resolves from [workspace.package]");
    assert_eq!(version, Some(Version::new(0, 3, 0)));
}

#[test]
fn does_not_inherit_a_workspace_version_from_above_the_working_root() {
    // The member inherits `version.workspace = true`, but the only
    // `[workspace.package].version` lives in a `Cargo.toml` ABOVE the repo root
    // (the working-directory trust boundary). Resolution must not climb past the
    // root, so the inherited version is unreachable and the read errors out —
    // rather than silently consulting a manifest outside the repository.
    let repo = SampleRepo::materialize("rust/workspace-inherited").expect("materialize");

    // Relocate the workspace-root manifest one level above the repo root so the
    // bounded ancestor walk can never reach it.
    let above_root = repo
        .root()
        .parent()
        .expect("repo root has a parent temp dir")
        .join("Cargo.toml");
    std::fs::rename(repo.child("Cargo.toml"), &above_root).expect("relocate workspace root");

    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let error = target()
        .declared_version(&app_module())
        .expect_err("inherited version above the working root must not resolve");
    assert!(
        error
            .to_string()
            .contains("no ancestor workspace root with [workspace.package].version was found"),
        "expected a bounded-resolution error, got: {error}"
    );
}

#[test]
fn package_builds_a_publishable_artifact() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = target();
    let module = app_module();

    let artifact = target.package(&module).expect("cargo package");
    // The implementation honors `CARGO_TARGET_DIR` / `build.target-dir`, so only
    // the target-dir-relative suffix is invariant — not the `target/` segment.
    assert!(artifact.path.ends_with("package/app-0.1.0.crate"));
}

#[test]
fn publish_surfaces_manifest_resolution_failures_before_cargo_runs() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = target();
    let mut module = app_module();
    module.manifest = Some(RepoPath::new("missing/Cargo.toml").unwrap());

    let error = target
        .publish(
            &module,
            &toven_ports::Artifact::new("ignored"),
            &toven_ports::ReleaseCredentials::default(),
            toven_ports::Visibility::Public,
        )
        .expect_err("missing manifest must fail fast before cargo runs");
    assert!(
        error.to_string().contains("does not exist"),
        "expected a typed manifest-existence error, got: {error}"
    );
}
