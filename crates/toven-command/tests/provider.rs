//! Behavioral tests for the command provider surface: configure, the
//! user-declared-only task table, toolchain-probe derivation, wizard detection,
//! and release-target gating.

use std::path::Path;

use rskit_config::RawValue;
use toven_command::CommandProvider;
use toven_ports::{ConfiguredAdapter, Provider, TaskKind};

fn provider() -> CommandProvider {
    CommandProvider::new().expect("provider")
}

fn raw_subtree(toml: &str) -> RawValue {
    rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
}

fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    let raw = raw_subtree(adapter_config);
    provider().configure(raw).expect("configure")
}

const DECLARED_MODULES: &str =
    include_str!("../../toven-testkit/fixtures/ecosystems/command/adapter/declared-modules.toml");
const WITH_TOOLCHAIN: &str =
    include_str!("../../toven-testkit/fixtures/ecosystems/command/adapter/with-toolchain.toml");
const MODULES_WITHOUT_TOOLCHAIN: &str = include_str!(
    "../../toven-testkit/fixtures/ecosystems/command/adapter/modules-without-toolchain.toml"
);

#[test]
fn only_user_declared_tasks_are_exposed_via_common_config() {
    let adapter = configure(DECLARED_MODULES);
    let tasks = &adapter.common().tasks;
    assert_eq!(tasks.len(), 2);

    let build = tasks.get("build").expect("build task");
    assert!(build.kind.is_none());
    assert_eq!(build.argv, ["make", "-C", "{module.root}", "build"]);
    assert_eq!(
        build
            .materialize("command", "build")
            .expect("materialize")
            .kind,
        TaskKind::Build
    );

    let deploy = tasks.get("deploy").expect("deploy task");
    assert!(deploy.kind.is_none());
    assert_eq!(deploy.argv, ["./scripts/deploy.sh", "{module.name}"]);
    assert_eq!(
        deploy
            .materialize("command", "deploy")
            .expect("materialize")
            .kind,
        TaskKind::Custom("deploy".to_string())
    );
}

#[test]
fn empty_section_yields_no_tasks() {
    let provider = provider();
    let raw = raw_subtree("");
    let adapter = provider.configure(raw).expect("configures");
    assert!(adapter.common().tasks.is_empty());
}

#[test]
fn toolchain_probe_prefers_declared_toolchain() {
    let adapter = configure(WITH_TOOLCHAIN);
    let probe = adapter.toolchain_probe();
    assert_eq!(probe.program, "bazel");
    assert_eq!(probe.args, ["version"]);
    assert_eq!(probe.label, "bazel-toolchain");
}

#[test]
fn toolchain_probe_defaults_to_first_task_program() {
    let adapter = configure(DECLARED_MODULES);
    let probe = adapter.toolchain_probe();
    assert_eq!(probe.program, "make");
    assert_eq!(probe.args, ["--version"]);
}

#[test]
fn configure_rejects_unknown_section_field() {
    let raw = raw_subtree("bogus = true");
    assert!(provider().configure(raw).is_err());
}

#[test]
fn configure_rejects_a_task_entry_without_argv() {
    let raw = raw_subtree("[tasks.test]\nargv = []\n");
    let Err(error) = provider().configure(raw) else {
        panic!("a task entry without argv must be rejected")
    };
    assert!(
        error.to_string().contains("ecosystems.command.tasks.test"),
        "{error}"
    );
}

#[test]
fn configure_rejects_modules_without_tasks_or_toolchain() {
    let raw = raw_subtree(MODULES_WITHOUT_TOOLCHAIN);
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
    let adapter = configure(DECLARED_MODULES);
    assert!(adapter.release_target().expect("ok").is_none());
}

#[test]
fn command_never_self_detects() {
    assert!(provider().detect(Path::new(".")).expect("ok").is_none());
}

#[test]
fn command_empty_render_round_trips() {
    let detection = toven_ports::Detection::bare(provider().ecosystem_id().clone());
    let questionnaire = provider().questionnaire(&detection).expect("questionnaire");
    assert!(questionnaire.is_empty());

    let fragment = provider()
        .render(&detection, &toven_ports::Answers::new())
        .expect("render");
    let rendered = toml::to_string(&fragment.table).expect("serialize fragment");
    let raw = raw_subtree(&rendered);
    provider()
        .configure(raw)
        .expect("rendered fragment configures");
}
