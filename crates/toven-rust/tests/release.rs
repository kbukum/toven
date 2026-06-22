//! Behavioral tests for the crates.io release target's version I/O and manifest
//! mutation. The target resolves a module's repo-relative manifest against the
//! process working directory, so these tests serialize a `set_current_dir` into
//! a materialized sample repo.

use std::sync::Mutex;

use rskit_version::semver::Version;
use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{ReleaseMutation, ReleaseTarget};
use toven_rust::CratesIoTarget;
use toven_testkit::SampleRepo;

/// Serializes the process-wide working-directory mutation across tests in this
/// binary.
static CWD_LOCK: Mutex<()> = Mutex::new(());

fn app_module() -> Module {
    let id = ModuleRef::new(EcosystemId::new("rust").unwrap(), "app").unwrap();
    let mut module = Module::new(id, RepoPath::new("crates/app").unwrap());
    module.package = Some("app".to_string());
    module.manifest = Some(RepoPath::new("crates/app/Cargo.toml").unwrap());
    module
}

#[test]
fn reads_the_declared_version_from_the_manifest() {
    let _guard = CWD_LOCK.lock().unwrap();
    let repo = SampleRepo::materialize("single-rust").expect("materialize");
    std::env::set_current_dir(repo.root()).expect("chdir");

    let version = CratesIoTarget::new()
        .declared_version(&app_module())
        .expect("declared version");
    assert_eq!(version, Version::new(0, 1, 0));
}

#[test]
fn apply_release_rewrites_the_declared_version() {
    let _guard = CWD_LOCK.lock().unwrap();
    let repo = SampleRepo::materialize("single-rust").expect("materialize");
    std::env::set_current_dir(repo.root()).expect("chdir");

    let target = CratesIoTarget::new();
    let module = app_module();
    target
        .apply_release(&module, &ReleaseMutation::version(Version::new(0, 2, 0)))
        .expect("apply release");

    let version = target.declared_version(&module).expect("re-read version");
    assert_eq!(version, Version::new(0, 2, 0));
}

#[test]
fn reads_a_version_inherited_from_the_workspace_root() {
    let _guard = CWD_LOCK.lock().unwrap();
    let repo = SampleRepo::materialize("workspace-inherited-rust").expect("materialize");
    std::env::set_current_dir(repo.root()).expect("chdir");

    let version = CratesIoTarget::new()
        .declared_version(&app_module())
        .expect("inherited version resolves from [workspace.package]");
    assert_eq!(version, Version::new(0, 3, 0));
}

#[test]
fn registry_facing_methods_are_deferred_not_faked() {
    let _guard = CWD_LOCK.lock().unwrap();
    let repo = SampleRepo::materialize("single-rust").expect("materialize");
    std::env::set_current_dir(repo.root()).expect("chdir");

    let target = CratesIoTarget::new();
    let module = app_module();
    assert!(target.published_versions(&module).is_err());
    assert!(target.package(&module).is_err());
    assert!(
        target
            .publish(&module, &toven_ports::Artifact::new("ignored"))
            .is_err()
    );
}
