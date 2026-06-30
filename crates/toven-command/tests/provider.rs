//! Behavioral tests for the command provider surface: configure, the
//! user-declared-only task table, toolchain-probe derivation, scaffolding, and
//! release-target gating. Configs come from testkit fixtures.

use std::path::Path;

use toven_command::CommandProvider;
use toven_ports::{ConfiguredAdapter, Provider, TaskKind};
use toven_testkit::fixtures;

fn provider() -> CommandProvider {
    CommandProvider::new().expect("provider")
}

fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    let raw_text = fixtures::ecosystem_string("command", adapter_config).expect("adapter fixture");
    let raw = toven_testkit::raw_subtree(&raw_text).expect("valid adapter toml");
    provider().configure(raw).expect("configure")
}

#[test]
fn only_user_declared_tasks_are_emitted() {
    let adapter = configure("adapter/declared-modules.toml");
    let tasks = adapter.default_tasks();
    assert_eq!(tasks.len(), 2);

    let build = tasks
        .iter()
        .find(|t| t.kind == TaskKind::Build)
        .expect("build task");
    assert!(build.name.is_none());
    assert_eq!(build.argv, ["make", "-C", "{module.root}", "build"]);

    let deploy = tasks
        .iter()
        .find(|t| t.kind == TaskKind::Custom("deploy".to_string()))
        .expect("deploy task");
    assert!(deploy.name.is_none());
    assert_eq!(deploy.argv, ["./scripts/deploy.sh", "{module.name}"]);
}

#[test]
fn empty_section_yields_no_tasks() {
    let provider = provider();
    let raw = toven_testkit::raw_subtree("").expect("subtree");
    let adapter = provider.configure(raw).expect("configures");
    assert!(adapter.default_tasks().is_empty());
}

#[test]
fn toolchain_probe_prefers_declared_toolchain() {
    let adapter = configure("adapter/with-toolchain.toml");
    let probe = adapter.toolchain_probe();
    assert_eq!(probe.program, "bazel");
    assert_eq!(probe.args, ["version"]);
    assert_eq!(probe.label, "bazel-toolchain");
}

#[test]
fn toolchain_probe_defaults_to_first_task_program() {
    let adapter = configure("adapter/declared-modules.toml");
    let probe = adapter.toolchain_probe();
    assert_eq!(probe.program, "make");
    assert_eq!(probe.args, ["--version"]);
}

#[test]
fn configure_rejects_unknown_section_field() {
    let raw = toven_testkit::raw_subtree("bogus = true").expect("subtree");
    assert!(provider().configure(raw).is_err());
}

#[test]
fn configure_rejects_modules_without_tasks_or_toolchain() {
    let raw_text = fixtures::ecosystem_string("command", "adapter/modules-without-toolchain.toml")
        .expect("adapter fixture");
    let raw = toven_testkit::raw_subtree(&raw_text).expect("valid adapter toml");
    let Err(error) = provider().configure(raw) else {
        panic!("modules without tasks or [toolchain] must be rejected");
    };
    assert!(
        error.to_string().contains("no tasks or [toolchain]"),
        "{error}"
    );
}

#[test]
fn command_never_offers_a_release_target() {
    let adapter = configure("adapter/declared-modules.toml");
    assert!(adapter.release_target().expect("ok").is_none());
}

#[test]
fn command_never_scaffolds() {
    assert!(provider().scaffold(Path::new(".")).expect("ok").is_none());
}
