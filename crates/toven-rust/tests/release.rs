//! Behavioral tests for the crates.io release target's version I/O and manifest
//! mutation. The target resolves a module's repo-relative manifest against the
//! process working directory, so these tests pin the working directory to a
//! materialized sample repo via a `CurrentDirGuard` — which serializes the
//! process-wide change and restores the previous directory on drop.

use rskit_version::semver::Version;
use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{ReleaseMutation, ReleaseTarget};
use toven_rust::CargoRegistryTarget;
use toven_testkit::{CurrentDirGuard, SampleRepo};

fn app_module() -> Module {
    let id = ModuleRef::new(EcosystemId::new("rust").unwrap(), "app").unwrap();
    let mut module = Module::new(id, RepoPath::new("crates/app").unwrap());
    module.package = Some("app".to_string());
    module.manifest = Some(RepoPath::new("crates/app/Cargo.toml").unwrap());
    module
}

#[test]
fn reads_the_declared_version_from_the_manifest() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let version = CargoRegistryTarget::new()
        .declared_version(&app_module())
        .expect("declared version");
    assert_eq!(version, Version::new(0, 1, 0));
}

#[test]
fn apply_release_rewrites_the_declared_version() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let target = CargoRegistryTarget::new();
    let module = app_module();
    target
        .apply_release(&module, &ReleaseMutation::version(Version::new(0, 2, 0)))
        .expect("apply release");

    let version = target.declared_version(&module).expect("re-read version");
    assert_eq!(version, Version::new(0, 2, 0));
}

#[test]
fn reads_a_version_inherited_from_the_workspace_root() {
    let repo = SampleRepo::materialize("rust/workspace-inherited").expect("materialize");
    let _cwd = CurrentDirGuard::change_to(repo.root()).expect("chdir");

    let version = CargoRegistryTarget::new()
        .declared_version(&app_module())
        .expect("inherited version resolves from [workspace.package]");
    assert_eq!(version, Version::new(0, 3, 0));
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

    let error = CargoRegistryTarget::new()
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

    let target = CargoRegistryTarget::new();
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

    let target = CargoRegistryTarget::new();
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
