//! The Rust wizard `render` step: author the complete `[ecosystems.rust]`
//! section, including the full cargo task table, from the user's answers.
//!
//! The config is the authoritative task source: `toven init` writes the whole
//! cargo command table into `toven.toml` at onboarding time, so the planner
//! reads runnable tasks straight from config rather than a compiled-in adapter
//! default.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use toml::Table;
use toven_ports::{
    Answers, CoverageConfig, Detection, EcosystemFragment, Enforcement, FanOut, ReleaseConfig,
    TaskEntry, TaskKind, release_config,
};

use crate::config::Manifests;
use crate::detect::RustFacts;
use crate::manifests::sibling_lockfile;
use crate::questionnaire::{MANIFESTS, RELEASE_REGISTRY, RUNNER_NEXTEST, TEST_RUNNER};

/// The serializable `[ecosystems.rust]` body: the managed workspace roots plus
/// the complete task table.
#[derive(Debug, Serialize)]
struct RustFragmentBody {
    manifests: Manifests,
    #[serde(skip_serializing_if = "CoverageConfig::is_default")]
    coverage: CoverageConfig,
    #[serde(skip_serializing_if = "ReleaseConfig::is_default")]
    release: ReleaseConfig,
    tasks: BTreeMap<String, TaskEntry>,
}

/// Render the complete `[ecosystems.rust]` fragment from a [`Detection`] and
/// the user's [`Answers`].
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

    let selected = selected_manifests(&facts, answers);
    let shared_inputs = shared_lockfiles(&selected, &facts);
    let body = RustFragmentBody {
        manifests: manifests_for(&selected, &facts),
        coverage: starter_coverage(),
        release: release_config(answers, Some(RELEASE_REGISTRY)),
        tasks: task_table(use_nextest, &shared_inputs),
    };
    let table = Table::try_from(&body).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to encode rust fragment").with_cause(error)
    })?;

    Ok(EcosystemFragment::new(detection.ecosystem.clone(), table))
}

/// Choose how to author the managed manifests.
///
/// Managing every discovered workspace is authored as `auto` so a workspace
/// added later is picked up without editing config. A narrowed selection is
/// frozen as an explicit list.
fn manifests_for(selected: &[String], facts: &RustFacts) -> Manifests {
    if selected == facts.manifests.as_slice() {
        Manifests::Auto
    } else {
        Manifests::Explicit(selected.to_vec())
    }
}

/// The existing sibling lockfiles of the selected workspaces, authored into
/// every task's `shared_inputs`. Only lockfiles the probe found on disk are
/// included, so an absent path never silently hashes to an empty digest.
fn shared_lockfiles(selected: &[String], facts: &RustFacts) -> Vec<String> {
    let mut locks: Vec<String> = selected
        .iter()
        .map(|manifest| sibling_lockfile(manifest))
        .filter(|lock| facts.lockfiles.contains(lock))
        .collect();
    locks.sort_unstable();
    locks.dedup();
    locks
}

/// The manifests to manage, resolved from the wizard answers.
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
fn task_table(use_nextest: bool, shared_inputs: &[String]) -> BTreeMap<String, TaskEntry> {
    let test_argv = if use_nextest {
        vec![
            "cargo".to_string(),
            "nextest".to_string(),
            "run".to_string(),
            // A crate with no test targets is not a failure; nextest otherwise exits non-zero with
            // "no tests to run". This flag is on a generated command, not user argv.
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
        fan_out_entry("build", FanOut::Batchable, shared_inputs),
    );
    tasks.insert(
        "check".to_string(),
        fan_out_entry("check", FanOut::Batchable, shared_inputs),
    );
    tasks.insert("format".to_string(), format_entry(shared_inputs));
    tasks.insert(
        "format-check".to_string(),
        format_check_entry(shared_inputs),
    );
    tasks.insert(
        "lint".to_string(),
        fan_out_entry("clippy", FanOut::Batchable, shared_inputs),
    );
    tasks.insert("vuln".to_string(), vuln_entry(shared_inputs));
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
            cacheable: true,
            fail_if_output: false,
            shared_inputs: shared_inputs.to_vec(),
        },
    );
    tasks.insert("doc".to_string(), doc_entry(shared_inputs));
    tasks.insert("run".to_string(), run_entry(shared_inputs));
    tasks.insert("coverage".to_string(), coverage_entry(shared_inputs));
    tasks
}

/// The starter `[ecosystems.rust].coverage` block onboarding authors.
///
/// It seeds the dimensions cargo-llvm-cov measures (line/function/region plus a
/// changed-scope line floor) at conservative floors under `advisory`
/// enforcement, so a fresh `toven coverage` reports a verdict without failing
/// CI until the user raises the floors and flips enforcement to `block`.
fn starter_coverage() -> CoverageConfig {
    CoverageConfig {
        line: Some(80.0),
        function: Some(80.0),
        region: Some(70.0),
        changed_line: Some(85.0),
        enforcement: Some(Enforcement::Advisory),
        ..CoverageConfig::default()
    }
}

/// The `coverage` measurement entry: `cargo llvm-cov` writes one workspace lcov
/// tracefile into Toven's staging dir, which the coverage verb aggregates and
/// gates. Tagged [`TaskKind::Coverage`](toven_ports::TaskKind::Coverage) for
/// cross-ecosystem recognition, and `cacheable = false` so every run
/// re-measures.
fn coverage_entry(shared_inputs: &[String]) -> TaskEntry {
    TaskEntry {
        kind: Some(TaskKind::Coverage),
        argv: vec![
            "cargo".to_string(),
            "llvm-cov".to_string(),
            "--manifest-path".to_string(),
            "{module.manifest}".to_string(),
            "--workspace".to_string(),
            "--lcov".to_string(),
            "--output-path".to_string(),
            "target/toven/coverage/rust-{module.name}.lcov".to_string(),
            "{args}".to_string(),
        ],
        selector: Vec::new(),
        fan_out: FanOut::WholeWorkspace,
        persistent: false,
        readiness: toven_ports::Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        cacheable: false,
        fail_if_output: false,
        shared_inputs: shared_inputs.to_vec(),
    }
}

/// The per-module selector every fan-out cargo task shares.
fn fan_out_selector() -> Vec<String> {
    vec!["-p".to_string(), "{module.package}".to_string()]
}

/// The argv for a cargo subcommand that fans out over modules.
fn fan_out_argv(subcommand: &str) -> Vec<String> {
    fan_out_argv_with(subcommand, &[])
}

/// Like [`fan_out_argv`], but inserts `extra` flags after the per-module
/// selector and before the `{args}` passthrough.
fn fan_out_argv_with(subcommand: &str, extra: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "cargo".to_string(),
        subcommand.to_string(),
        "--manifest-path".to_string(),
        "{module.manifest}".to_string(),
        "{module.selector}".to_string(),
    ];
    argv.extend(extra.iter().map(|flag| (*flag).to_string()));
    argv.push("{args}".to_string());
    argv
}

/// The `doc` entry: `cargo doc --no-deps` documents only the project's own
/// crates. Dependency documentation is noise a user never wants from
/// `toven doc`, so `--no-deps` is the natural default rather than something the
/// author must add by hand.
fn doc_entry(shared_inputs: &[String]) -> TaskEntry {
    let mut entry = fan_out_entry("doc", FanOut::Batchable, shared_inputs);
    entry.argv = fan_out_argv_with("doc", &["--no-deps"]);
    entry
}

/// A fan-out cargo task entry (`-p {module.package}` selector, lockfile
/// inputs).
fn fan_out_entry(subcommand: &str, fan_out: FanOut, shared_inputs: &[String]) -> TaskEntry {
    TaskEntry {
        kind: None,
        argv: fan_out_argv(subcommand),
        selector: fan_out_selector(),
        fan_out,
        persistent: false,
        readiness: toven_ports::Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        cacheable: true,
        fail_if_output: false,
        shared_inputs: shared_inputs.to_vec(),
    }
}

/// The whole-workspace `cargo fmt --all` entry (no per-module selector).
/// `extra` flags are inserted after `--all`, before the `{args}` passthrough.
fn whole_workspace_entry(subcommand: &str, extra: &[&str], shared_inputs: &[String]) -> TaskEntry {
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
        cacheable: true,
        fail_if_output: false,
        shared_inputs: shared_inputs.to_vec(),
    }
}

/// The CI-friendly `cargo fmt --all --check` entry: verifies formatting without
/// rewriting the tree. Tagged
/// [`TaskKind::Format`](toven_ports::TaskKind::Format) so it keeps the format
/// run-strategy and cross-ecosystem recognition despite its distinct
/// `format-check` name.
fn format_check_entry(shared_inputs: &[String]) -> TaskEntry {
    let mut entry = whole_workspace_entry("fmt", &["--check"], shared_inputs);
    entry.kind = Some(toven_ports::TaskKind::Format);
    entry
}

/// The mutating `cargo fmt --all` entry. Authored `cacheable = false`: a
/// formatter rewrites the tree, so a stale content-key hit must never suppress
/// a reformat (the `format-check` twin keeps caching its non-mutating verify).
fn format_entry(shared_inputs: &[String]) -> TaskEntry {
    let mut entry = whole_workspace_entry("fmt", &[], shared_inputs);
    entry.cacheable = false;
    entry
}

/// The `vuln` supply-chain entry: `cargo audit --file {lockfile}` once per
/// workspace lockfile. Fanning out whole-workspace hits each `Cargo.lock`
/// exactly once (the resolved graph an audit needs), the direct analog of Go's
/// per-module `govulncheck`. `cargo audit` reads the `RustSec` advisory DB with
/// no project config, so it stays generic where `cargo deny` needs a per-repo
/// policy file. The `vuln` name resolves to [`toven_ports::TaskKind::Vuln`], so
/// it inherits the unordered run strategy without an explicit `kind` (matching
/// `build`/`check`).
fn vuln_entry(shared_inputs: &[String]) -> TaskEntry {
    TaskEntry {
        kind: None,
        argv: vec![
            "cargo".to_string(),
            "audit".to_string(),
            "--file".to_string(),
            "{workspace.root}/Cargo.lock".to_string(),
            "{args}".to_string(),
        ],
        selector: Vec::new(),
        fan_out: FanOut::WholeWorkspace,
        persistent: false,
        readiness: toven_ports::Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        cacheable: true,
        fail_if_output: false,
        shared_inputs: shared_inputs.to_vec(),
    }
}
fn run_entry(shared_inputs: &[String]) -> TaskEntry {
    let mut entry = fan_out_entry("run", FanOut::PerModule, shared_inputs);
    entry.persistent = true;
    entry
}

#[cfg(test)]
mod tests {
    use rskit_cli::ChoiceId;
    use toml::Table;
    use toven_ports::{Answer, Answers, Detection, FanOut};

    use super::render;
    use crate::config::{Manifests, RustConfig};
    use crate::detect::RustFacts;
    use crate::questionnaire::TEST_RUNNER;

    fn detection(nextest: bool) -> Detection {
        let facts = RustFacts {
            manifests: vec!["Cargo.toml".to_string()],
            lockfiles: vec!["Cargo.lock".to_string()],
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
            lockfiles: vec![
                "contrib/Cargo.lock".to_string(),
                "core/Cargo.lock".to_string(),
                "examples/Cargo.lock".to_string(),
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
        // A crate with no test targets must not be a failure; the generated nextest
        // command carries `--no-tests=pass` so an empty crate is `ok`.
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
                "coverage",
                "doc",
                "format",
                "format-check",
                "lint",
                "run",
                "test",
                "vuln"
            ]
        );
        assert_eq!(config.manifests, Manifests::Auto);
        let run = config.common.tasks.get("run").expect("run task");
        assert!(run.persistent);
        assert_eq!(run.fan_out, FanOut::PerModule);
        let format = config.common.tasks.get("format").expect("format task");
        assert_eq!(format.fan_out, FanOut::WholeWorkspace);
        assert!(format.selector.is_empty());
        assert!(!format.argv.contains(&"--check".to_string()));
        assert!(!format.cacheable, "the mutating format twin must not cache");
        let format_check = config
            .common
            .tasks
            .get("format-check")
            .expect("format-check task");
        assert_eq!(format_check.fan_out, FanOut::WholeWorkspace);
        assert!(format_check.selector.is_empty());
        assert!(format_check.argv.contains(&"--check".to_string()));
        assert!(
            format_check.cacheable,
            "the non-mutating format-check twin caches"
        );
        assert_eq!(
            format_check.resolved_kind("format-check"),
            toven_ports::TaskKind::Format
        );
        let vuln = config.common.tasks.get("vuln").expect("vuln task");
        assert_eq!(vuln.argv[..2], ["cargo", "audit"]);
        assert_eq!(vuln.fan_out, FanOut::WholeWorkspace);
        assert!(vuln.selector.is_empty());
        assert!(vuln.cacheable);
        assert_eq!(vuln.resolved_kind("vuln"), toven_ports::TaskKind::Vuln);
        let doc = config.common.tasks.get("doc").expect("doc task");
        assert_eq!(doc.argv[..2], ["cargo", "doc"]);
        assert!(
            doc.argv.contains(&"--no-deps".to_string()),
            "doc documents only the project's own crates, never dependencies"
        );
    }

    #[test]
    fn authors_a_coverage_task_and_starter_block() {
        let fragment = render(&detection(true), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let coverage = config.common.tasks.get("coverage").expect("coverage task");
        assert_eq!(coverage.argv[..2], ["cargo", "llvm-cov"]);
        assert!(coverage.argv.iter().any(|arg| arg == "--lcov"));
        assert!(
            coverage
                .argv
                .contains(&"target/toven/coverage/rust-{module.name}.lcov".to_string())
        );
        assert_eq!(coverage.fan_out, FanOut::WholeWorkspace);
        assert!(coverage.selector.is_empty());
        assert!(!coverage.cacheable, "coverage re-measures every run");
        assert_eq!(
            coverage.resolved_kind("coverage"),
            toven_ports::TaskKind::Coverage
        );
        // The starter block seeds the cargo-llvm-cov dimensions under advisory.
        assert_eq!(config.common.coverage.line, Some(80.0));
        assert_eq!(config.common.coverage.function, Some(80.0));
        assert_eq!(config.common.coverage.region, Some(70.0));
        assert_eq!(config.common.coverage.changed_line, Some(85.0));
        assert_eq!(
            config.common.coverage.enforcement,
            Some(toven_ports::Enforcement::Advisory)
        );
        config
            .common
            .coverage
            .validate("ecosystems.rust.coverage")
            .expect("starter coverage validates");
    }

    #[test]
    fn omits_the_release_block_when_not_opted_in() {
        let fragment = render(&detection(true), &Answers::new()).expect("render");
        assert!(
            !fragment.table.contains_key("release"),
            "no release block is authored unless the user opts in",
        );
        let config = parse(&fragment);
        assert!(config.common.release.is_default());
    }

    #[test]
    fn opting_in_authors_a_crates_io_release_block() {
        let answers = Answers::new()
            .with(toven_ports::RELEASE_ENABLED, Answer::Bool(true))
            .with(
                toven_ports::RELEASE_PRERELEASE,
                Answer::MultiChoice(vec![ChoiceId::new("alpha")]),
            )
            .with(toven_ports::RELEASE_HOST, Answer::Bool(true));
        let fragment = render(&detection(true), &answers).expect("render");
        let config = parse(&fragment);

        assert_eq!(config.common.release.registry.as_deref(), Some("crates-io"));
        assert_eq!(
            config
                .common
                .release
                .prerelease
                .as_ref()
                .expect("prerelease")
                .channels,
            ["alpha"]
        );
        assert_eq!(
            config
                .common
                .release
                .host
                .as_ref()
                .expect("host")
                .forge
                .as_deref(),
            Some("github")
        );
        config
            .common
            .release
            .validate("ecosystems.rust.release")
            .expect("authored release validates");
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
    fn managing_every_discovered_workspace_is_authored_as_auto() {
        let fragment = render(&multi_detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        assert_eq!(config.manifests, Manifests::Auto);
    }

    #[test]
    fn every_task_shares_the_existing_lockfiles() {
        let fragment = render(&multi_detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        for (name, entry) in &config.common.tasks {
            assert_eq!(
                entry.shared_inputs,
                [
                    "contrib/Cargo.lock",
                    "core/Cargo.lock",
                    "examples/Cargo.lock"
                ],
                "task '{name}' shares the per-workspace lockfiles",
            );
        }
    }

    #[test]
    fn manifest_selection_is_frozen_as_an_explicit_list() {
        let answers = Answers::new().with(
            crate::questionnaire::MANIFESTS,
            Answer::MultiChoice(vec![
                ChoiceId::new("examples/Cargo.toml"),
                ChoiceId::new("core/Cargo.toml"),
            ]),
        );
        let fragment = render(&multi_detection(), &answers).expect("render");
        let config = parse(&fragment);
        assert_eq!(
            config.manifests,
            Manifests::Explicit(vec![
                "core/Cargo.toml".to_string(),
                "examples/Cargo.toml".to_string()
            ])
        );
        // shared_inputs narrow to the selected workspaces' lockfiles.
        let build = config.common.tasks.get("build").expect("build task");
        assert_eq!(
            build.shared_inputs,
            ["core/Cargo.lock", "examples/Cargo.lock"]
        );
    }

    #[test]
    fn deselecting_every_manifest_falls_back_to_auto() {
        let answers = Answers::new().with(
            crate::questionnaire::MANIFESTS,
            Answer::MultiChoice(Vec::new()),
        );
        let fragment = render(&multi_detection(), &answers).expect("render");
        let config = parse(&fragment);
        assert_eq!(config.manifests, Manifests::Auto);
    }
}
