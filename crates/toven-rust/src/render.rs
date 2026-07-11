//! The Rust wizard `render` step: author the complete `[ecosystems.rust]`
//! section, including the full cargo task table, from the user's answers.
//!
//! The config is the authoritative task source: `toven init` writes the whole
//! cargo command table into `toven.toml` at onboarding time, so the planner reads
//! runnable tasks straight from config rather than a compiled-in adapter default.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use toml::Table;
use toven_ports::{Answers, Detection, EcosystemFragment, FanOut, TaskEntry};

use crate::detect::RustFacts;
use crate::questionnaire::{MANIFESTS, RUNNER_NEXTEST, TEST_RUNNER};

/// The workspace-level cache input every cargo task shares: the lockfile pins the
/// resolved dependency versions, so a change invalidates every task.
const SHARED_LOCKFILE: &str = "Cargo.lock";

/// The serializable `[ecosystems.rust]` body: the discovered manifests plus the
/// complete task table.
#[derive(Debug, Serialize)]
struct RustFragmentBody {
    manifests: Vec<String>,
    tasks: BTreeMap<String, TaskEntry>,
}

/// Render the complete `[ecosystems.rust]` fragment from a [`Detection`] and the
/// user's [`Answers`].
///
/// The `test` task's argv follows the chosen runner (`cargo nextest run …` or
/// `cargo test …`); every other task is authored with the canonical cargo
/// templates. When no runner answer is present (non-interactive with no
/// recommendation resolved), the detected default is used.
///
/// # Errors
/// Propagates a facts-decode failure or a TOML encoding failure.
pub(crate) fn render(detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment> {
    let facts = RustFacts::from_detection(detection)?;
    let use_nextest = match answers.choice(&TEST_RUNNER.into()) {
        Some(choice) => choice.as_str() == RUNNER_NEXTEST,
        None => facts.nextest,
    };

    let body = RustFragmentBody {
        manifests: selected_manifests(&facts, answers),
        tasks: task_table(use_nextest),
    };
    let table = Table::try_from(&body).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to encode rust fragment").with_cause(error)
    })?;

    Ok(EcosystemFragment::new(detection.ecosystem.clone(), table))
}

/// The manifests to author into the fragment.
///
/// When the wizard asked which workspaces to manage, the user's selection is
/// honored in discovered order; deselecting everything falls back to the full
/// discovered set so onboarding never yields a manifest-less config. With no
/// such question (a single discovered manifest), the discovered set is used.
fn selected_manifests(facts: &RustFacts, answers: &Answers) -> Vec<String> {
    let Some(selected) = answers.multi_choice(&MANIFESTS.into()) else {
        return facts.manifests.clone();
    };
    let chosen: Vec<String> = facts
        .manifests
        .iter()
        .filter(|manifest| {
            selected
                .iter()
                .any(|choice| choice.as_str() == manifest.as_str())
        })
        .cloned()
        .collect();
    if chosen.is_empty() {
        facts.manifests.clone()
    } else {
        chosen
    }
}

/// Build the complete cargo task table, honoring the chosen test runner.
fn task_table(use_nextest: bool) -> BTreeMap<String, TaskEntry> {
    let test_argv = if use_nextest {
        vec![
            "cargo".to_string(),
            "nextest".to_string(),
            "run".to_string(),
            // A crate with no test targets is not a failure; nextest otherwise
            // exits non-zero with "no tests to run". This flag is on a generated
            // command, not user argv.
            "--no-tests=pass".to_string(),
            "--manifest-path".to_string(),
            "{module.manifest}".to_string(),
            "{module.selector}".to_string(),
            "{args}".to_string(),
        ]
    } else {
        fan_out_argv("test")
    };

    let mut tasks = BTreeMap::new();
    tasks.insert(
        "build".to_string(),
        fan_out_entry("build", FanOut::Batchable),
    );
    tasks.insert(
        "check".to_string(),
        fan_out_entry("check", FanOut::Batchable),
    );
    tasks.insert("format".to_string(), whole_workspace_entry("fmt", &[]));
    tasks.insert("format-check".to_string(), format_check_entry());
    tasks.insert(
        "lint".to_string(),
        fan_out_entry("clippy", FanOut::Batchable),
    );
    tasks.insert(
        "test".to_string(),
        TaskEntry {
            kind: None,
            argv: test_argv,
            selector: fan_out_selector(),
            fan_out: FanOut::Batchable,
            persistent: false,
            readiness: toven_ports::Readiness::Started,
            readiness_timeout_secs: None,
            cache_args: false,
            shared_inputs: vec![SHARED_LOCKFILE.to_string()],
        },
    );
    tasks.insert("doc".to_string(), fan_out_entry("doc", FanOut::Batchable));
    tasks.insert("run".to_string(), run_entry());
    tasks
}

/// The per-module selector every fan-out cargo task shares.
fn fan_out_selector() -> Vec<String> {
    vec!["-p".to_string(), "{module.package}".to_string()]
}

/// The argv for a cargo subcommand that fans out over modules.
fn fan_out_argv(subcommand: &str) -> Vec<String> {
    vec![
        "cargo".to_string(),
        subcommand.to_string(),
        "--manifest-path".to_string(),
        "{module.manifest}".to_string(),
        "{module.selector}".to_string(),
        "{args}".to_string(),
    ]
}

/// A fan-out cargo task entry (`-p {module.package}` selector, lockfile input).
fn fan_out_entry(subcommand: &str, fan_out: FanOut) -> TaskEntry {
    TaskEntry {
        kind: None,
        argv: fan_out_argv(subcommand),
        selector: fan_out_selector(),
        fan_out,
        persistent: false,
        readiness: toven_ports::Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        shared_inputs: vec![SHARED_LOCKFILE.to_string()],
    }
}

/// The whole-workspace `cargo fmt --all` entry (no per-module selector). `extra`
/// flags are inserted after `--all`, before the `{args}` passthrough.
fn whole_workspace_entry(subcommand: &str, extra: &[&str]) -> TaskEntry {
    let mut argv = vec![
        "cargo".to_string(),
        subcommand.to_string(),
        "--manifest-path".to_string(),
        "{module.manifest}".to_string(),
        "--all".to_string(),
    ];
    argv.extend(extra.iter().map(|flag| (*flag).to_string()));
    argv.push("{args}".to_string());
    TaskEntry {
        kind: None,
        argv,
        selector: Vec::new(),
        fan_out: FanOut::WholeWorkspace,
        persistent: false,
        readiness: toven_ports::Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        shared_inputs: vec![SHARED_LOCKFILE.to_string()],
    }
}

/// The CI-friendly `cargo fmt --all --check` entry: verifies formatting without
/// rewriting the tree. Tagged [`TaskKind::Format`](toven_ports::TaskKind::Format)
/// so it keeps the format run-strategy and cross-ecosystem recognition despite
/// its distinct `format-check` name.
fn format_check_entry() -> TaskEntry {
    let mut entry = whole_workspace_entry("fmt", &["--check"]);
    entry.kind = Some(toven_ports::TaskKind::Format);
    entry
}

/// The persistent `cargo run` entry (per-module, long-lived).
fn run_entry() -> TaskEntry {
    let mut entry = fan_out_entry("run", FanOut::PerModule);
    entry.persistent = true;
    entry
}

#[cfg(test)]
mod tests {
    use rskit_cli::ChoiceId;
    use toml::Table;
    use toven_ports::{Answer, Answers, Detection, FanOut};

    use super::render;
    use crate::config::RustConfig;
    use crate::detect::RustFacts;
    use crate::questionnaire::TEST_RUNNER;

    fn detection(nextest: bool) -> Detection {
        let facts = RustFacts {
            manifests: vec!["Cargo.toml".to_string()],
            nextest,
        };
        Detection::new(
            toven_model::EcosystemId::new("rust").unwrap(),
            Table::try_from(&facts).unwrap(),
        )
    }

    fn multi_detection() -> Detection {
        let facts = RustFacts {
            manifests: vec![
                "core/Cargo.toml".to_string(),
                "contrib/Cargo.toml".to_string(),
                "examples/Cargo.toml".to_string(),
            ],
            nextest: true,
        };
        Detection::new(
            toven_model::EcosystemId::new("rust").unwrap(),
            Table::try_from(&facts).unwrap(),
        )
    }

    fn parse(fragment: &toven_ports::EcosystemFragment) -> RustConfig {
        fragment
            .table
            .clone()
            .try_into()
            .expect("fragment parses back through RustConfig")
    }

    #[test]
    fn nextest_answer_authors_a_nextest_test_task() {
        let answers = Answers::new().with(TEST_RUNNER, Answer::Choice(ChoiceId::new("nextest")));
        let fragment = render(&detection(true), &answers).expect("render");
        let config = parse(&fragment);
        let test = config.common.tasks.get("test").expect("test task");
        assert_eq!(
            test.argv[..4],
            ["cargo", "nextest", "run", "--no-tests=pass"]
        );
        assert_eq!(test.fan_out, FanOut::Batchable);
        assert_eq!(test.shared_inputs, ["Cargo.lock"]);
    }

    #[test]
    fn cargo_test_answer_authors_a_cargo_test_task() {
        let answers = Answers::new().with(TEST_RUNNER, Answer::Choice(ChoiceId::new("cargo-test")));
        let fragment = render(&detection(false), &answers).expect("render");
        let config = parse(&fragment);
        let test = config.common.tasks.get("test").expect("test task");
        assert_eq!(test.argv[..2], ["cargo", "test"]);
    }

    #[test]
    fn no_answer_falls_back_to_detected_runner() {
        let fragment = render(&detection(true), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let test = config.common.tasks.get("test").expect("test task");
        assert_eq!(
            test.argv[..4],
            ["cargo", "nextest", "run", "--no-tests=pass"]
        );
    }

    #[test]
    fn nextest_test_task_passes_empty_crates_instead_of_failing() {
        // A crate with no test targets must not be a failure; the generated
        // nextest command carries `--no-tests=pass` so an empty crate is `ok`.
        let fragment = render(&detection(true), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let test = config.common.tasks.get("test").expect("test task");
        assert!(test.argv.iter().any(|arg| arg == "--no-tests=pass"));
    }

    #[test]
    fn authors_the_complete_task_table() {
        let fragment = render(&detection(false), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let mut names: Vec<&str> = config.common.tasks.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "build",
                "check",
                "doc",
                "format",
                "format-check",
                "lint",
                "run",
                "test"
            ]
        );
        assert_eq!(config.manifests, ["Cargo.toml"]);
        let run = config.common.tasks.get("run").expect("run task");
        assert!(run.persistent);
        assert_eq!(run.fan_out, FanOut::PerModule);
        let format = config.common.tasks.get("format").expect("format task");
        assert_eq!(format.fan_out, FanOut::WholeWorkspace);
        assert!(format.selector.is_empty());
        assert!(!format.argv.contains(&"--check".to_string()));
        let format_check = config
            .common
            .tasks
            .get("format-check")
            .expect("format-check task");
        assert_eq!(format_check.fan_out, FanOut::WholeWorkspace);
        assert!(format_check.selector.is_empty());
        assert!(format_check.argv.contains(&"--check".to_string()));
        assert_eq!(
            format_check.resolved_kind("format-check"),
            toven_ports::TaskKind::Format
        );
    }

    #[test]
    fn rendered_table_round_trips_through_materialize() {
        let fragment = render(&detection(true), &Answers::new()).expect("render");
        let config = parse(&fragment);
        for (key, entry) in &config.common.tasks {
            entry
                .materialize("rust", key)
                .expect("every authored entry materializes");
        }
    }

    #[test]
    fn discovered_manifests_are_authored_when_no_selection() {
        let fragment = render(&multi_detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        assert_eq!(
            config.manifests,
            [
                "core/Cargo.toml",
                "contrib/Cargo.toml",
                "examples/Cargo.toml"
            ]
        );
    }

    #[test]
    fn manifest_selection_is_honored_in_discovered_order() {
        let answers = Answers::new().with(
            crate::questionnaire::MANIFESTS,
            Answer::MultiChoice(vec![
                ChoiceId::new("examples/Cargo.toml"),
                ChoiceId::new("core/Cargo.toml"),
            ]),
        );
        let fragment = render(&multi_detection(), &answers).expect("render");
        let config = parse(&fragment);
        assert_eq!(config.manifests, ["core/Cargo.toml", "examples/Cargo.toml"]);
    }

    #[test]
    fn deselecting_every_manifest_falls_back_to_all_discovered() {
        let answers = Answers::new().with(
            crate::questionnaire::MANIFESTS,
            Answer::MultiChoice(Vec::new()),
        );
        let fragment = render(&multi_detection(), &answers).expect("render");
        let config = parse(&fragment);
        assert_eq!(
            config.manifests,
            [
                "core/Cargo.toml",
                "contrib/Cargo.toml",
                "examples/Cargo.toml"
            ]
        );
    }
}
