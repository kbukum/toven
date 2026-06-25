//! Behavioral tests for the provider surface: configure, default tasks,
//! scaffolding, and release-target gating. Configs come from testkit fixtures.

use rskit_fs::TempDir;
use toven_go::GoProvider;
use toven_ports::{Provider, RunStrategy, TaskKind};
use toven_testkit::fixtures;

fn provider() -> GoProvider {
    GoProvider::new().expect("provider")
}

fn configure(adapter_config: &str) -> Box<dyn toven_ports::ConfiguredAdapter> {
    let raw_text = fixtures::ecosystem_string("go", adapter_config).expect("adapter fixture");
    let raw: toml::Value = toml::from_str(&raw_text).expect("valid adapter toml");
    provider().configure(raw).expect("configure")
}

#[test]
fn default_tasks_cover_every_builtin_kind() {
    let adapter = configure("adapter/single-module.toml");
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
fn test_task_renders_a_go_argv() {
    let adapter = configure("adapter/single-module.toml");
    let test = adapter
        .default_tasks()
        .into_iter()
        .find(|t| t.kind == TaskKind::Test)
        .expect("test task");
    assert_eq!(test.argv.first().map(String::as_str), Some("go"));
    assert!(test.argv.iter().any(|arg| arg == "test"));
    assert!(test.argv.iter().any(|arg| arg == "{module.root}"));
    assert_eq!(test.selector, ["./..."]);
    assert_eq!(test.shared_inputs, ["go.sum"]);
}

#[test]
fn configure_accepts_the_flattened_common_knobs() {
    // `deny_unknown_fields` on the outer struct must still admit the flattened
    // engine-common knobs (`run_strategy`, `[tasks.*]`). This locks in the
    // fragile serde flatten behavior the adapter relies on.
    let adapter = configure("adapter/gotestsum.toml");
    let common = adapter.common();

    assert_eq!(common.run_strategy, Some(RunStrategy::LeafToTop));
    assert!(
        common.tasks.contains_key("test"),
        "flattened task override should be parsed"
    );
}

#[test]
fn configure_rejects_an_unknown_section_field() {
    let adapter = fixtures::ecosystem_string("go", "adapter/single-module.toml").unwrap();
    let mut raw: toml::Table = toml::from_str(&adapter).unwrap();
    raw.insert("bogus".to_string(), toml::Value::Boolean(true));
    assert!(provider().configure(toml::Value::Table(raw)).is_err());
}

#[test]
fn go_has_no_release_target() {
    let adapter = configure("adapter/single-module.toml");
    assert!(adapter.release_target().expect("release target").is_none());
}

#[test]
fn scaffold_detects_a_go_module() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/x\n\ngo 1.26\n",
    )
    .expect("write go.mod");
    let fragment = provider()
        .scaffold(dir.path())
        .expect("scaffold")
        .expect("go module detected");
    assert_eq!(fragment.ecosystem.as_str(), "go");
    assert!(fragment.table.contains_key("modules"));
}

#[test]
fn scaffold_skips_a_non_go_directory() {
    let dir = TempDir::new().expect("temp dir");
    assert!(provider().scaffold(dir.path()).expect("scaffold").is_none());
}
