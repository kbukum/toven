//! The JavaScript wizard `render` step: author the `[ecosystems.js]` task
//! table.
//!
//! Every task runs one of the packages' own scripts through the detected
//! package manager, once per package that declares the script (`requires =
//! ["script:<name>"]`). A root-only script such as a repo-wide `lint` therefore
//! runs once, on the root package. An `install` task per root runs the frozen
//! install; Toven never installs implicitly.
//!
//! When every package's `test` script calls one recognized runner, a
//! `coverage` task runs that script with the runner's LCOV flags, writing
//! `target/toven/coverage/js-<module>/lcov.info`, plus an advisory starter
//! `coverage` block.
//!
//! Every cacheable task lists the roots' install inputs (root manifest,
//! lockfile, manager settings) as `shared_inputs`, so a dependency bump
//! invalidates each package's cache.
//!
//! For now every task fans out per package; batching several packages into one
//! package-manager call is not built yet.

use std::collections::{BTreeMap, BTreeSet};

use rskit_cli::ChoiceId;
use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use toml::Table;
use toven_model::Capability;
use toven_ports::{
    Answers, CoverageConfig, Detection, EcosystemFragment, Enforcement, FanOut, Readiness,
    TaskDependency, TaskEntry, TaskKind,
};

use crate::config::Roots;
use crate::detect::{BUILD_SCRIPT, JsFacts, TEST_SCRIPT};
use crate::package_manager::Manager;
use crate::questionnaire::{DEPENDENCY_BUILDS, GATES};
use crate::test_runner::TestRunner;

/// The script names onboarding recommends as gates.
pub(crate) const STANDARD_SCRIPTS: [&str; 8] = [
    "build",
    "typecheck",
    "lint",
    "format",
    "format-check",
    "format:check",
    "test",
    "dev",
];

/// The task that installs each root's dependencies.
const INSTALL_TASK: &str = "install";

/// The task that measures coverage.
const COVERAGE_TASK: &str = "coverage";

/// The directory a package's coverage run writes `lcov.info` into: Toven's
/// coverage staging directory, under the `js-<module>` producer name the
/// coverage reader resolves relative paths against.
const COVERAGE_REPORT_DIR: &str = "{project.root}/target/toven/coverage/js-{module.name}";

/// The directory a package's build writes, when its entry points name it.
const DIST: &str = "dist";

/// The serializable `[ecosystems.js]` body.
#[derive(Debug, Serialize)]
struct JsFragmentBody {
    roots: Roots,
    #[serde(skip_serializing_if = "CoverageConfig::is_default")]
    coverage: CoverageConfig,
    tasks: BTreeMap<String, TaskEntry>,
}

/// The task name for `script`: `:` becomes `-` (`format:check` →
/// `format-check`). `None` when the name holds anything other than ASCII
/// letters, digits, `-`, and `_`, or would replace the `install` task; such a
/// script is not offered and can be authored by hand.
pub(crate) fn task_name(script: &str) -> Option<String> {
    let name = script.replace(':', "-");
    let valid = !name.is_empty()
        && name != INSTALL_TASK
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    valid.then_some(name)
}

/// Render the `[ecosystems.js]` fragment from a [`Detection`] and the user's
/// [`Answers`].
///
/// # Errors
/// A facts-decode failure, two selected scripts that map to one task name, or
/// a TOML encoding failure.
pub(crate) fn render(detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment> {
    let facts = JsFacts::from_detection(detection)?;
    let manager = facts.manager()?;

    let selected: Vec<&String> = match answers.multi_choice(&GATES.into()) {
        Some(chosen) => {
            let chosen: BTreeSet<&str> = chosen.iter().map(ChoiceId::as_str).collect();
            facts
                .scripts
                .iter()
                .filter(|script| chosen.contains(script.as_str()))
                .collect()
        }
        None => facts
            .scripts
            .iter()
            .filter(|script| STANDARD_SCRIPTS.contains(&script.as_str()))
            .collect(),
    };
    let dependency_builds = selected.iter().any(|script| *script == BUILD_SCRIPT)
        && answers.bool(&DEPENDENCY_BUILDS.into()).unwrap_or(true);

    let mut tasks = BTreeMap::new();
    let mut sources: BTreeMap<String, &str> = BTreeMap::new();
    let coverage = if let Some(runner) = facts.test_runner
        && selected.iter().any(|script| *script == TEST_SCRIPT)
    {
        tasks.insert(
            COVERAGE_TASK.to_string(),
            coverage_entry(manager, runner, dependency_builds)?,
        );
        starter_coverage()
    } else {
        CoverageConfig::default()
    };
    for script in selected {
        let Some(name) = task_name(script) else {
            continue;
        };
        if let Some(other) = sources.insert(name.clone(), script) {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!(
                    "scripts '{other}' and '{script}' both become the js task '{name}'; select \
                     only one of them"
                ),
            ));
        }
        let mut entry = script_entry(manager, script, dependency_builds, facts.dist)?;
        if entry.cacheable {
            entry.shared_inputs.clone_from(&facts.install_inputs);
        }
        tasks.insert(name, entry);
    }
    tasks.insert(INSTALL_TASK.to_string(), install_entry(manager));

    let table = Table::try_from(&JsFragmentBody {
        roots: Roots::Auto,
        coverage,
        tasks,
    })
    .map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to encode js fragment").with_cause(error)
    })?;
    Ok(EcosystemFragment::new(detection.ecosystem.clone(), table))
}

/// The task running `script` in each package that declares it.
///
/// Standard gates are cacheable: their output depends only on the package's
/// inputs. `format` rewrites files and `dev` keeps running, so neither caches;
/// any other script may deploy, publish, or reach the network, so it does not
/// cache either.
fn script_entry(
    manager: Manager,
    script: &str,
    dependency_builds: bool,
    dist: bool,
) -> AppResult<TaskEntry> {
    let mut entry = base_entry(
        manager.run_script("{module.root}", script, &[]),
        FanOut::PerModule,
    );
    entry
        .requires
        .insert(Capability::new(format!("script:{script}"))?);
    match script {
        "build" => {
            if dist {
                entry.outputs.push(DIST.to_string());
            }
        }
        "typecheck" => entry.kind = Some(TaskKind::Check),
        "format-check" | "format:check" => entry.kind = Some(TaskKind::Format),
        "dev" => {
            entry.kind = Some(TaskKind::Run);
            entry.persistent = true;
            entry.cacheable = false;
        }
        "lint" | "test" => {}
        // `format` rewrites files; any other script may have side effects.
        _ => entry.cacheable = false,
    }
    if dependency_builds && matches!(script, "build" | "test" | "typecheck") {
        entry.depends_on.push(TaskDependency::parse("^build")?);
    }
    Ok(entry)
}

/// The `coverage` task: each package's `test` script with the runner's LCOV
/// flags. It re-measures every run, so it never caches.
fn coverage_entry(
    manager: Manager,
    runner: TestRunner,
    dependency_builds: bool,
) -> AppResult<TaskEntry> {
    let mut entry = base_entry(
        manager.run_script(
            "{module.root}",
            TEST_SCRIPT,
            &runner.lcov_args(COVERAGE_REPORT_DIR),
        ),
        FanOut::PerModule,
    );
    entry.kind = Some(TaskKind::Coverage);
    entry.cacheable = false;
    entry
        .requires
        .insert(Capability::new(format!("script:{TEST_SCRIPT}"))?);
    if dependency_builds {
        entry.depends_on.push(TaskDependency::parse("^build")?);
    }
    Ok(entry)
}

/// The starter `[ecosystems.js].coverage` block: line and changed-line floors
/// under `advisory`, so a fresh `toven coverage` reports without failing CI.
/// Function and branch floors are left unset because `bun test` reports no
/// branches and runners count functions differently.
fn starter_coverage() -> CoverageConfig {
    CoverageConfig {
        line: Some(80.0),
        changed_line: Some(85.0),
        enforcement: Some(Enforcement::Advisory),
        ..CoverageConfig::default()
    }
}

/// The `install` task: the frozen-lockfile install, once per root.
fn install_entry(manager: Manager) -> TaskEntry {
    let mut entry = base_entry(
        manager.in_dir(Some("{workspace.root}"), manager.frozen_install()),
        FanOut::WholeWorkspace,
    );
    entry.cacheable = false;
    entry.workspace_closure = true;
    entry
}

/// A cacheable task entry with no extras.
const fn base_entry(argv: Vec<String>, fan_out: FanOut) -> TaskEntry {
    TaskEntry {
        kind: None,
        argv,
        selector: Vec::new(),
        fan_out,
        requires: BTreeSet::new(),
        depends_on: Vec::new(),
        outputs: Vec::new(),
        workspace_closure: false,
        persistent: false,
        readiness: Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        cacheable: true,
        fail_if_output: false,
        shared_inputs: Vec::new(),
        modules: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use rskit_cli::ChoiceId;
    use toml::{Table, Value};
    use toven_model::EcosystemId;
    use toven_ports::{Answer, Answers, Detection};

    use super::{render, task_name};
    use crate::questionnaire::{DEPENDENCY_BUILDS, GATES};

    fn detection(manager: &str, scripts: &[&str], dist: bool) -> Detection {
        let mut facts = Table::new();
        facts.insert("manager".into(), manager.into());
        facts.insert(
            "scripts".into(),
            Value::Array(scripts.iter().map(|s| (*s).into()).collect()),
        );
        facts.insert("dist".into(), dist.into());
        Detection::new(EcosystemId::new("js").unwrap(), facts)
    }

    fn tasks(detection: &Detection, answers: &Answers) -> Table {
        let fragment = render(detection, answers).expect("renders");
        fragment.table["tasks"].as_table().unwrap().clone()
    }

    fn strings(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect()
    }

    fn gates(scripts: &[&str]) -> Answers {
        Answers::new().with(
            GATES,
            Answer::MultiChoice(scripts.iter().map(|s| ChoiceId::new(*s)).collect()),
        )
    }

    #[test]
    fn defaults_render_standard_scripts_with_dependency_builds() {
        let detection = detection(
            "npm",
            &[
                "build",
                "deploy",
                "dev",
                "format",
                "format:check",
                "lint",
                "test",
                "typecheck",
            ],
            true,
        );
        let fragment = render(&detection, &Answers::new()).unwrap();
        assert_eq!(fragment.table["roots"].as_str(), Some("auto"));
        let tasks = tasks(&detection, &Answers::new());
        assert_eq!(
            tasks.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "build",
                "dev",
                "format",
                "format-check",
                "install",
                "lint",
                "test",
                "typecheck"
            ]
        );

        let build = tasks["build"].as_table().unwrap();
        assert_eq!(
            strings(&build["argv"]),
            [
                "npm",
                "--prefix",
                "{module.root}",
                "run",
                "build",
                "--",
                "{args}"
            ]
        );
        assert_eq!(strings(&build["requires"]), ["script:build"]);
        assert_eq!(strings(&build["depends_on"]), ["^build"]);
        assert_eq!(strings(&build["outputs"]), ["dist"]);
        assert_eq!(build["fan_out"].as_str(), Some("per-module"));
        for name in ["test", "typecheck"] {
            assert_eq!(strings(&tasks[name]["depends_on"]), ["^build"], "{name}");
        }
        assert_eq!(tasks["typecheck"]["kind"].as_str(), Some("check"));
        assert!(tasks["lint"].get("depends_on").is_none());
        assert_eq!(tasks["format"]["cacheable"].as_bool(), Some(false));
        assert_eq!(tasks["format-check"]["kind"].as_str(), Some("format"));
        assert_eq!(strings(&tasks["format-check"]["argv"])[4], "format:check");
        assert_eq!(tasks["dev"]["persistent"].as_bool(), Some(true));
        assert_eq!(tasks["dev"]["kind"].as_str(), Some("run"));

        let install = tasks["install"].as_table().unwrap();
        assert_eq!(
            strings(&install["argv"]),
            ["npm", "--prefix", "{workspace.root}", "ci"]
        );
        assert_eq!(install["fan_out"].as_str(), Some("whole-workspace"));
        assert_eq!(install["cacheable"].as_bool(), Some(false));
    }

    #[test]
    fn answers_select_scripts_and_turn_off_dependency_builds() {
        let detection = detection("bun", &["build", "deploy", "test"], false);
        let answers =
            gates(&["deploy", "test", "build"]).with(DEPENDENCY_BUILDS, Answer::Bool(false));
        let tasks = tasks(&detection, &answers);
        assert_eq!(
            strings(&tasks["test"]["argv"]),
            ["bun", "run", "--cwd", "{module.root}", "test", "{args}"]
        );
        assert!(tasks["test"].get("depends_on").is_none());
        assert!(tasks["build"].get("outputs").is_none());
        assert_eq!(tasks["deploy"]["cacheable"].as_bool(), Some(false));
        assert_eq!(
            strings(&tasks["install"]["argv"]),
            [
                "bun",
                "install",
                "--cwd",
                "{workspace.root}",
                "--frozen-lockfile"
            ]
        );
    }

    #[test]
    fn dependency_builds_need_a_selected_build() {
        let tasks = tasks(
            &detection("pnpm", &["build", "test"], false),
            &gates(&["test"]),
        );
        assert!(tasks.get("build").is_none());
        assert!(tasks["test"].get("depends_on").is_none());
    }

    #[test]
    fn a_shared_test_runner_renders_a_coverage_task_and_starter_block() {
        let mut detection = detection("pnpm", &["build", "test"], true);
        detection
            .facts
            .insert("test_runner".into(), "vitest".into());
        let fragment = render(&detection, &Answers::new()).unwrap();
        let coverage = fragment.table["coverage"].as_table().unwrap();
        assert_eq!(coverage["line"].as_float(), Some(80.0));
        assert_eq!(coverage["changed_line"].as_float(), Some(85.0));
        assert_eq!(coverage["enforcement"].as_str(), Some("advisory"));

        let tasks = tasks(&detection, &Answers::new());
        let task = tasks["coverage"].as_table().unwrap();
        assert_eq!(
            strings(&task["argv"]),
            [
                "pnpm",
                "--dir",
                "{module.root}",
                "run",
                "test",
                "--coverage.enabled",
                "--coverage.reporter=lcovonly",
                "--coverage.reportsDirectory={project.root}/target/toven/coverage/js-{module.name}",
                "{args}"
            ]
        );
        assert_eq!(task["kind"].as_str(), Some("coverage"));
        assert_eq!(task["cacheable"].as_bool(), Some(false));
        assert_eq!(strings(&task["requires"]), ["script:test"]);
        assert_eq!(strings(&task["depends_on"]), ["^build"]);
        assert_eq!(task["fan_out"].as_str(), Some("per-module"));
    }

    #[test]
    fn no_coverage_task_without_a_runner_or_a_test_gate() {
        let plain = detection("npm", &["test"], false);
        let fragment = render(&plain, &Answers::new()).unwrap();
        assert!(fragment.table.get("coverage").is_none());
        assert!(tasks(&plain, &Answers::new()).get("coverage").is_none());

        let mut runner = detection("npm", &["lint", "test"], false);
        runner.facts.insert("test_runner".into(), "jest".into());
        let lint_only = tasks(&runner, &gates(&["lint"]));
        assert!(lint_only.get("coverage").is_none());
    }

    #[test]
    fn cacheable_tasks_hash_the_install_inputs() {
        let mut detection = detection("bun", &["build", "format", "test"], false);
        detection.facts.insert(
            "install_inputs".into(),
            Value::Array(vec!["bun.lock".into(), "package.json".into()]),
        );
        let tasks = tasks(&detection, &Answers::new());
        for name in ["build", "test"] {
            assert_eq!(
                strings(&tasks[name]["shared_inputs"]),
                ["bun.lock", "package.json"]
            );
        }
        for name in ["format", "install"] {
            assert!(tasks[name].get("shared_inputs").is_none(), "{name}");
        }
    }

    #[test]
    fn yarn_berry_installs_immutably() {
        let tasks = tasks(&detection("yarn-berry", &[], false), &Answers::new());
        assert_eq!(
            strings(&tasks["install"]["argv"]),
            [
                "yarn",
                "--cwd",
                "{workspace.root}",
                "install",
                "--immutable"
            ]
        );
    }

    #[test]
    fn two_scripts_for_one_task_conflict() {
        let detection = detection("npm", &["format-check", "format:check"], false);
        let error = render(&detection, &Answers::new()).expect_err("conflict");
        assert!(error.to_string().contains("format-check"), "{error}");
    }

    #[test]
    fn task_names_are_safe_identifiers() {
        assert_eq!(task_name("test:e2e").as_deref(), Some("test-e2e"));
        assert_eq!(task_name("weird name"), None);
        assert_eq!(task_name("install"), None);
    }
}
