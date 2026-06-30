//! Behavioral tests for the provider surface: configure, default tasks,
//! scaffolding, and release-target gating. Configs come from testkit fixtures.

use rskit_fs::TempDir;
use toven_ports::{Provider, RunStrategy, TaskKind};
use toven_rust::RustProvider;
use toven_testkit::{SampleRepo, fixtures};

fn provider() -> RustProvider {
    RustProvider::new().expect("provider")
}

fn configure(adapter_config: &str) -> Box<dyn toven_ports::ConfiguredAdapter> {
    let raw_text = fixtures::ecosystem_string("rust", adapter_config).expect("adapter fixture");
    let raw = toven_testkit::raw_subtree(&raw_text).expect("valid adapter toml");
    provider().configure(raw).expect("configure")
}

#[test]
fn default_tasks_cover_every_builtin_kind() {
    let adapter = configure("adapter/single-manifest.toml");
    let kinds: Vec<TaskKind> = adapter
        .default_tasks()
        .into_iter()
        .map(|t| t.kind)
        .collect();

    for expected in [
        TaskKind::Build,
        TaskKind::Check,
        TaskKind::Format,
        TaskKind::Lint,
        TaskKind::Test,
        TaskKind::Doc,
        TaskKind::Run,
    ] {
        assert!(
            kinds.contains(&expected),
            "missing default task for {expected:?}"
        );
    }
}

#[test]
fn test_task_renders_a_cargo_argv() {
    let adapter = configure("adapter/single-manifest.toml");
    let test = adapter
        .default_tasks()
        .into_iter()
        .find(|t| t.kind == TaskKind::Test)
        .expect("test task");
    assert_eq!(test.argv.first().map(String::as_str), Some("cargo"));
    assert!(test.argv.iter().any(|arg| arg == "test"));
    assert_eq!(test.selector, ["-p", "{module.package}"]);
    assert_eq!(test.shared_inputs, ["Cargo.lock"]);
}

#[test]
fn configure_accepts_the_flattened_common_knobs() {
    // `deny_unknown_fields` on the outer struct must still admit the flattened
    // engine-common knobs (`run_strategy`, `[release]`, `[tasks.*]`). This locks
    // in the fragile serde flatten behavior the adapter relies on.
    let adapter = configure("adapter/cargo.toml");
    let common = adapter.common();

    assert_eq!(common.run_strategy, Some(RunStrategy::LeafToTop));
    assert_eq!(common.release.registry.as_deref(), Some("crates-io"));
    assert!(
        common.tasks.contains_key("test"),
        "flattened task override should be parsed"
    );
}

#[test]
fn configure_rejects_an_unknown_section_field() {
    let adapter = fixtures::ecosystem_string("rust", "adapter/single-manifest.toml").unwrap();
    let raw = toven_testkit::raw_subtree(&format!("{adapter}\nbogus = true\n")).expect("subtree");
    assert!(provider().configure(raw).is_err());
}

#[test]
fn publishable_config_exposes_a_release_target() {
    let adapter = configure("adapter/single-manifest.toml");
    assert!(adapter.release_target().expect("release target").is_some());
}

#[test]
fn unpublished_config_has_no_release_target() {
    let adapter = configure("adapter/unpublished.toml");
    assert!(adapter.release_target().expect("release target").is_none());
}

#[test]
fn scaffold_detects_a_cargo_project() {
    let repo = SampleRepo::materialize("single-rust").expect("materialize repo");
    let fragment = provider()
        .scaffold(repo.root())
        .expect("scaffold")
        .expect("cargo project detected");
    assert_eq!(fragment.ecosystem.as_str(), "rust");
    assert!(fragment.table.contains_key("manifests"));
}

#[test]
fn scaffold_skips_a_non_cargo_directory() {
    let dir = TempDir::new().expect("temp dir");
    assert!(provider().scaffold(dir.path()).expect("scaffold").is_none());
}
