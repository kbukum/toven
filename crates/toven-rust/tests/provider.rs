//! Behavioral tests for the provider surface: configure, the init wizard
//! (detect → questionnaire → render), and release-target gating. Configs come
//! from testkit fixtures.

use rskit_fs::TempDir;
use toven_ports::{Answer, Answers, Provider, QuestionKind, RunStrategy};
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
fn configure_reads_the_authoritative_task_table() {
    // The runnable tasks now live in the config, not a compiled-in default: the
    // adapter exposes them via `common().tasks`.
    let adapter = configure("adapter/cargo.toml");
    assert!(
        adapter.common().tasks.contains_key("test"),
        "authored task table should be parsed"
    );
}

#[test]
fn configure_accepts_the_flattened_common_knobs() {
    // `deny_unknown_fields` on the outer struct must still admit the flattened
    // engine-common knobs (`run_strategy`, `[release]`, `[tasks.*]`). This locks in
    // the fragile serde flatten behavior the adapter relies on.
    let adapter = configure("adapter/cargo.toml");
    let common = adapter.common();

    assert_eq!(common.run_strategy, Some(RunStrategy::LeafToTop));
    assert_eq!(common.release.registry.as_deref(), Some("crates-io"));
}

#[test]
fn configure_rejects_an_unknown_section_field() {
    let adapter = fixtures::ecosystem_string("rust", "adapter/single-manifest.toml").unwrap();
    let raw = toven_testkit::raw_subtree(&format!("{adapter}\nbogus = true\n")).expect("subtree");
    assert!(provider().configure(raw).is_err());
}

#[test]
fn configure_rejects_a_task_entry_without_argv() {
    let raw = toven_testkit::raw_subtree("[tasks.test]\nargv = []\n").expect("subtree");
    let Err(error) = provider().configure(raw) else {
        panic!("a task entry without argv must be rejected")
    };
    assert!(
        error.to_string().contains("ecosystems.rust.tasks.test"),
        "{error}"
    );
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
fn wizard_detects_and_renders_a_cargo_project() {
    let repo = SampleRepo::materialize("rust/single").expect("materialize repo");

    let detection = provider()
        .detect(repo.root())
        .expect("detect")
        .expect("cargo project detected");
    assert_eq!(detection.ecosystem.as_str(), "rust");

    // The questionnaire asks the test-runner question and preselects a runner.
    let questionnaire = provider().questionnaire(&detection).expect("questionnaire");
    let question = &questionnaire.questions[0];
    let QuestionKind::Select(choices) = &question.kind else {
        panic!("expected a select question");
    };
    let recommended = choices
        .iter()
        .find(|choice| choice.is_recommended())
        .expect("a recommended choice");

    // Rendering with the recommended answer yields a complete, parseable section.
    let answers = Answers::new().with(
        question.id.clone(),
        Answer::Choice(recommended.id().clone()),
    );
    let fragment = provider().render(&detection, &answers).expect("render");
    assert_eq!(fragment.ecosystem.as_str(), "rust");
    assert!(fragment.table.contains_key("manifests"));
    assert!(fragment.table.contains_key("tasks"));

    // The rendered fragment configures back through the provider cleanly.
    let rendered = toml::to_string(&fragment.table).expect("serialize fragment");
    let raw = toven_testkit::raw_subtree(&rendered).expect("raw table");
    provider()
        .configure(raw)
        .expect("rendered fragment configures");
}

#[test]
fn wizard_skips_a_non_cargo_directory() {
    let dir = TempDir::new().expect("temp dir");
    assert!(provider().detect(dir.path()).expect("detect").is_none());
}
