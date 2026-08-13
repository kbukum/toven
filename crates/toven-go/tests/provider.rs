//! Behavioral tests for the provider surface: configure, the init wizard
//! (detect → questionnaire → render), and release-target gating.

use std::sync::Arc;

use rskit_config::RawValue;
use rskit_fs::TempDir;
use toven_go::GoProvider;
use toven_ports::{Provider, RunStrategy};
use toven_testkit::doubles::FakeToolRunner;

fn provider() -> GoProvider {
    GoProvider::new(Arc::new(FakeToolRunner::new())).expect("provider")
}

fn raw_subtree(toml: &str) -> RawValue {
    rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
}

fn configure(adapter_config: &str) -> Box<dyn toven_ports::ConfiguredAdapter> {
    let raw = raw_subtree(adapter_config);
    provider().configure(raw).expect("configure")
}

const SINGLE_MODULE: &str =
    include_str!("../../toven-testkit/fixtures/ecosystems/go/adapter/single-module.toml");
const GOTESTSUM: &str =
    include_str!("../../toven-testkit/fixtures/ecosystems/go/adapter/gotestsum.toml");

#[test]
fn configure_reads_the_authoritative_task_table() {
    let adapter = configure(GOTESTSUM);
    assert!(
        adapter.common().tasks.contains_key("test"),
        "authored task table should be parsed"
    );
}

#[test]
fn configure_accepts_the_flattened_common_knobs() {
    // `deny_unknown_fields` on the outer struct must still admit the flattened
    // engine-common knobs (`run_strategy`, `[tasks.*]`). This locks in the fragile
    // serde flatten behavior the adapter relies on.
    let adapter = configure(GOTESTSUM);
    let common = adapter.common();

    assert_eq!(common.run_strategy, Some(RunStrategy::LeafToTop));
    assert!(
        common.tasks.contains_key("test"),
        "flattened task table should be parsed"
    );
}

#[test]
fn configure_rejects_an_unknown_section_field() {
    let raw = raw_subtree(&format!("{SINGLE_MODULE}\nbogus = true\n"));
    assert!(provider().configure(raw).is_err());
}

#[test]
fn configure_rejects_a_task_entry_without_argv() {
    let raw = raw_subtree("[tasks.test]\nargv = []\n");
    let Err(error) = provider().configure(raw) else {
        panic!("a task entry without argv must be rejected")
    };
    assert!(
        error.to_string().contains("ecosystems.go.tasks.test"),
        "{error}"
    );
}

#[test]
fn go_exposes_release_target() {
    let adapter = configure(SINGLE_MODULE);
    let reader = toven_testkit::doubles::FakeVcsReader::new();
    assert!(
        adapter
            .release_target(&reader)
            .expect("release target")
            .is_some()
    );
}

#[test]
fn wizard_detects_and_renders_a_go_project() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/x\n\ngo 1.26\n",
    )
    .expect("write go.mod");

    let detection = provider()
        .detect(dir.path())
        .expect("detect")
        .expect("go project detected");
    assert_eq!(detection.ecosystem.as_str(), "go");

    let questionnaire = provider().questionnaire(&detection).expect("questionnaire");
    assert!(
        !questionnaire.is_empty(),
        "the wizard now offers linter/formatter/runner/hardening selections"
    );

    let fragment = provider()
        .render(&detection, &toven_ports::Answers::new())
        .expect("render");
    assert_eq!(fragment.ecosystem.as_str(), "go");
    assert!(fragment.table.contains_key("modules"));
    assert!(fragment.table.contains_key("tasks"));

    let rendered = toml::to_string(&fragment.table).expect("serialize fragment");
    let raw = raw_subtree(&rendered);
    provider()
        .configure(raw)
        .expect("rendered fragment configures");
}

#[test]
fn wizard_skips_a_non_go_directory() {
    let dir = TempDir::new().expect("temp dir");
    assert!(provider().detect(dir.path()).expect("detect").is_none());
}
