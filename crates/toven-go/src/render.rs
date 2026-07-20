//! The Go wizard `render` step: author the complete `[ecosystems.go]` section.
//!
//! The config is the authoritative task source: `toven init` writes the whole
//! `go` command table into `toven.toml` at onboarding time, so the planner
//! reads runnable tasks straight from config rather than a compiled-in adapter
//! default.
//!
//! The table is shaped by the wizard [`Answers`]: the lint backend, formatter,
//! test runner, and test-hardening choices each steer one or more task argvs.
//! Every task falls into one of two roles — a **non-mutating gate** (cacheable,
//! CI-safe) or a **mutating `*-fix` twin** (authored `cacheable = false` so a
//! stale content-key hit can never suppress the mutation on a later run).

use std::collections::BTreeMap;

use rskit_cli::ChoiceId;
use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use toml::Table;
use toven_ports::{
    Answers, CoverageConfig, Detection, EcosystemFragment, Enforcement, FanOut, Readiness,
    ReleaseConfig, TaskEntry, TaskKind, release_config,
};

use crate::config::Modules;
use crate::detect::GoFacts;
use crate::questionnaire::{
    FORMAT_GOFUMPT, FORMAT_GOIMPORTS, FORMATTER, LINT_BACKEND, LINT_GOLANGCI, LINT_STATICCHECK,
    RUNNER_GOTESTSUM, TEST_HARDENING, TEST_RUNNER,
};

/// The workspace-level cache inputs every `go` task shares.
const SHARED_INPUTS: [&str; 3] = ["go.sum", "go.work", "go.work.sum"];

/// The package pattern a per-module `go` invocation targets.
const PACKAGE_PATTERN: &str = "./...";

/// The resolved formatter program (`gofmt` / `gofumpt` / `goimports`).
struct Formatter {
    program: &'static str,
}

/// The resolved lint backend, or `None` when the built-in `go vet` was chosen
/// (in which case the `check` task already covers it and no `lint` task
/// exists).
enum LintBackend {
    Golangci,
    Staticcheck,
}

/// The serializable `[ecosystems.go]` body: module selection plus task table.
#[derive(Debug, Serialize)]
struct GoFragmentBody {
    modules: Modules,
    #[serde(skip_serializing_if = "CoverageConfig::is_default")]
    coverage: CoverageConfig,
    #[serde(skip_serializing_if = "ReleaseConfig::is_default")]
    release: ReleaseConfig,
    tasks: BTreeMap<String, TaskEntry>,
}

/// Render the complete `[ecosystems.go]` fragment from a [`Detection`] and the
/// user's [`Answers`].
///
/// # Errors
/// Propagates a facts-decode failure or a TOML encoding failure.
pub(crate) fn render(detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment> {
    GoFacts::from_detection(detection)?;

    let selections = Selections::from_answers(answers);
    let body = GoFragmentBody {
        modules: Modules::Auto,
        coverage: starter_coverage(),
        release: release_config(answers, None),
        tasks: task_table(&selections),
    };
    let table = Table::try_from(&body).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to encode go fragment").with_cause(error)
    })?;

    Ok(EcosystemFragment::new(detection.ecosystem.clone(), table))
}

/// The resolved wizard choices that steer the task argvs.
struct Selections {
    lint: Option<LintBackend>,
    formatter: Formatter,
    gotestsum: bool,
    harden_tests: bool,
}

impl Selections {
    /// Resolve every selection from the answers, falling back to the
    /// toolchain-native default (no external dependency) for any unanswered
    /// one.
    fn from_answers(answers: &Answers) -> Self {
        let lint = match answers.choice(&LINT_BACKEND.into()).map(ChoiceId::as_str) {
            Some(LINT_GOLANGCI) => Some(LintBackend::Golangci),
            Some(LINT_STATICCHECK) => Some(LintBackend::Staticcheck),
            // The built-in `go vet` (or no answer) authors no separate `lint` task — the `check`
            // task already runs `go vet`.
            _ => None,
        };
        let formatter = match answers.choice(&FORMATTER.into()).map(ChoiceId::as_str) {
            Some(FORMAT_GOFUMPT) => Formatter { program: "gofumpt" },
            Some(FORMAT_GOIMPORTS) => Formatter {
                program: "goimports",
            },
            _ => Formatter { program: "gofmt" },
        };
        let gotestsum =
            answers.choice(&TEST_RUNNER.into()).map(ChoiceId::as_str) == Some(RUNNER_GOTESTSUM);
        let harden_tests = answers.bool(&TEST_HARDENING.into()).unwrap_or(false);
        Self {
            lint,
            formatter,
            gotestsum,
            harden_tests,
        }
    }
}

/// Build the complete canonical Go task table from the resolved selections.
fn task_table(selections: &Selections) -> BTreeMap<String, TaskEntry> {
    let mut tasks = BTreeMap::new();
    tasks.insert("build".to_string(), go_module_entry("build"));
    tasks.insert("check".to_string(), go_module_entry("vet"));
    if let Some(lint) = &selections.lint {
        tasks.insert("lint".to_string(), lint_entry(lint));
    }
    tasks.insert(
        "format".to_string(),
        format_entry(&selections.formatter, /* fix */ true),
    );
    tasks.insert(
        "format-check".to_string(),
        format_entry(&selections.formatter, /* fix */ false),
    );
    tasks.insert("tidy".to_string(), tidy_entry(/* fix */ false));
    tasks.insert("tidy-fix".to_string(), tidy_entry(/* fix */ true));
    tasks.insert("vuln".to_string(), vuln_entry());
    tasks.insert("test".to_string(), test_entry(selections));
    tasks.insert("run".to_string(), run_entry());
    tasks.insert("coverage".to_string(), coverage_entry());
    tasks
}

/// The starter `[ecosystems.go].coverage` block onboarding authors.
///
/// Go's `-coverprofile` reports line coverage only, so the starter seeds the
/// line and changed-scope line floors (never function/region) at conservative
/// values under `advisory`, so a fresh `toven coverage` reports a verdict
/// without failing CI until the user raises the floors and flips enforcement to
/// `block`.
fn starter_coverage() -> CoverageConfig {
    CoverageConfig {
        line: Some(80.0),
        changed_line: Some(85.0),
        enforcement: Some(Enforcement::Advisory),
        ..CoverageConfig::default()
    }
}

/// The `coverage` measurement entry: `go test -coverprofile` writes one
/// workspace coverprofile into Toven's staging dir, which the coverage verb
/// aggregates and gates. Tagged [`TaskKind::Coverage`] for cross-ecosystem
/// recognition, and `cacheable = false` so every run re-measures.
fn coverage_entry() -> TaskEntry {
    let mut entry = base_entry(
        vec![
            "go".to_string(),
            "test".to_string(),
            "-coverprofile=target/toven/coverage/go-{module.name}.out".to_string(),
            "{args}".to_string(),
            PACKAGE_PATTERN.to_string(),
        ],
        Vec::new(),
        FanOut::WholeWorkspace,
    );
    entry.kind = Some(TaskKind::Coverage);
    entry.cacheable = false;
    entry
}

/// The workspace-level shared cache inputs as an owned vector.
fn shared_inputs() -> Vec<String> {
    SHARED_INPUTS
        .iter()
        .map(|input| (*input).to_string())
        .collect()
}

/// A minimal per-module cacheable task entry with the shared workspace inputs.
fn base_entry(argv: Vec<String>, selector: Vec<String>, fan_out: FanOut) -> TaskEntry {
    TaskEntry {
        kind: None,
        argv,
        selector,
        fan_out,
        persistent: false,
        readiness: Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        cacheable: true,
        fail_if_output: false,
        shared_inputs: shared_inputs(),
    }
}

/// A `go -C {module.root} <subcommand> {args} ./...` entry fanning per module.
fn go_module_entry(subcommand: &str) -> TaskEntry {
    base_entry(
        vec![
            "go".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            subcommand.to_string(),
            "{args}".to_string(),
            "{module.selector}".to_string(),
        ],
        vec![PACKAGE_PATTERN.to_string()],
        FanOut::PerModule,
    )
}

/// The `lint` task for the chosen external backend. Both run from the repo root
/// and scope to the module via a repo-relative package pattern
/// (`./{module.root}/…`), since neither `golangci-lint` nor `staticcheck`
/// accepts a `-C` chdir flag.
fn lint_entry(backend: &LintBackend) -> TaskEntry {
    let argv = match backend {
        LintBackend::Golangci => vec![
            "golangci-lint".to_string(),
            "run".to_string(),
            // golangci-lint guards its shared cache with a global per-user lock; this flag lets
            // Toven's per-module fan-out run linters concurrently.
            "--allow-parallel-runners".to_string(),
            "{args}".to_string(),
            "{module.selector}".to_string(),
        ],
        LintBackend::Staticcheck => vec![
            "staticcheck".to_string(),
            "{args}".to_string(),
            "{module.selector}".to_string(),
        ],
    };
    // The `./` prefix keeps this a filesystem-relative package pattern: Go tooling
    // reads a bare `svc/api/...` as an import path and matches no packages.
    base_entry(
        argv,
        vec![format!("./{{module.root}}/...")],
        FanOut::PerModule,
    )
}

/// The `format` (mutating) / `format-check` (non-mutating) entry.
///
/// Both run the chosen formatter once per workspace over `{workspace.root}`
/// (`gofmt` has no failing check mode and no per-module chdir, so a
/// whole-workspace pass — like `cargo fmt --all` — is the clean shape).
/// `format` rewrites files (`-w`) and is `cacheable = false` (a mutating task
/// cannot cache correctly). `format-check` lists offenders (`-l`); list-mode
/// formatters print the files that would change but still exit `0`, so it is
/// authored `fail_if_output = true` — the executor turns any stdout into a
/// failure, making the check a real CI gate while staying non-mutating and
/// cacheable.
fn format_entry(formatter: &Formatter, fix: bool) -> TaskEntry {
    let flag = if fix { "-w" } else { "-l" };
    let mut entry = base_entry(
        vec![
            formatter.program.to_string(),
            flag.to_string(),
            "{args}".to_string(),
            "{workspace.root}".to_string(),
        ],
        Vec::new(),
        FanOut::WholeWorkspace,
    );
    // Both twins keep the Format recognition (run-strategy + cross-ecosystem
    // fan-out matching) despite the `format-check` name.
    entry.kind = Some(TaskKind::Format);
    entry.cacheable = !fix;
    // The check twin gates on the offender list `-l` prints; the mutating twin
    // rewrites in place and produces no gating output.
    entry.fail_if_output = !fix;
    entry
}

/// The `tidy` (non-mutating) / `tidy-fix` (mutating) module-hygiene entry.
///
/// `tidy` runs `go mod tidy -diff` (Go 1.23+), which exits non-zero when
/// `go.mod` / `go.sum` would change — a CI-safe verification. `tidy-fix`
/// applies the edit and is `cacheable = false`. Both fan out per module because
/// `go.work` makes a workspace-wide `go mod tidy` ambiguous; each module is
/// tidied independently.
fn tidy_entry(fix: bool) -> TaskEntry {
    let mut argv = vec![
        "go".to_string(),
        "-C".to_string(),
        "{module.root}".to_string(),
        "mod".to_string(),
        "tidy".to_string(),
    ];
    if !fix {
        argv.push("-diff".to_string());
    }
    argv.push("{args}".to_string());
    let mut entry = base_entry(argv, Vec::new(), FanOut::PerModule);
    entry.cacheable = !fix;
    entry
}

/// The `vuln` supply-chain entry: `govulncheck -C {module.root} ./...` per
/// module. The `vuln` name resolves to [`TaskKind::Vuln`], so it inherits the
/// unordered run strategy without an explicit `kind` (matching
/// `build`/`check`/`lint`).
fn vuln_entry() -> TaskEntry {
    base_entry(
        vec![
            "govulncheck".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            "{args}".to_string(),
            "{module.selector}".to_string(),
        ],
        vec![PACKAGE_PATTERN.to_string()],
        FanOut::PerModule,
    )
}

/// The `test` entry for the chosen runner, optionally hardened with the race
/// detector and shuffled ordering.
fn test_entry(selections: &Selections) -> TaskEntry {
    let hardening: &[&str] = if selections.harden_tests {
        &["-race", "-shuffle=on"]
    } else {
        &[]
    };

    let argv = if selections.gotestsum {
        // `gotestsum` forwards everything after `--` to `go test`; `-C` chdirs into the
        // module the same way `go -C … test` does.
        let mut argv = vec![
            "gotestsum".to_string(),
            "--format".to_string(),
            "pkgname".to_string(),
            "--".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
        ];
        argv.extend(hardening.iter().map(|flag| (*flag).to_string()));
        argv.push("{args}".to_string());
        argv.push("{module.selector}".to_string());
        argv
    } else {
        let mut argv = vec![
            "go".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            "test".to_string(),
        ];
        argv.extend(hardening.iter().map(|flag| (*flag).to_string()));
        argv.push("{args}".to_string());
        argv.push("{module.selector}".to_string());
        argv
    };

    base_entry(argv, vec![PACKAGE_PATTERN.to_string()], FanOut::PerModule)
}

/// The persistent `go run .` entry.
fn run_entry() -> TaskEntry {
    let mut entry = base_entry(
        vec![
            "go".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            "run".to_string(),
            ".".to_string(),
            "{args}".to_string(),
        ],
        Vec::new(),
        FanOut::PerModule,
    );
    entry.persistent = true;
    entry
}

#[cfg(test)]
mod tests {
    use rskit_cli::ChoiceId;
    use toml::Table;
    use toven_ports::{Answer, Answers, Detection, FanOut, TaskKind};

    use super::render;
    use crate::config::{GoConfig, Modules};
    use crate::detect::GoFacts;
    use crate::questionnaire::{
        FORMATTER, LINT_BACKEND, RUNNER_GOTESTSUM, TEST_HARDENING, TEST_RUNNER,
    };

    fn detection() -> Detection {
        let facts = GoFacts {
            manifest: "go.mod".to_string(),
            golangci: false,
        };
        Detection::new(
            toven_model::EcosystemId::new("go").unwrap(),
            Table::try_from(&facts).unwrap(),
        )
    }

    fn parse(fragment: &toven_ports::EcosystemFragment) -> GoConfig {
        fragment
            .table
            .clone()
            .try_into()
            .expect("fragment parses back through GoConfig")
    }

    fn render_with(answers: &Answers) -> GoConfig {
        let fragment = render(&detection(), answers).expect("render");
        parse(&fragment)
    }

    #[test]
    fn default_answers_author_the_vet_only_catalog_without_a_lint_task() {
        let config = render_with(&Answers::new());
        let mut names: Vec<&str> = config.common.tasks.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "build",
                "check",
                "coverage",
                "format",
                "format-check",
                "run",
                "test",
                "tidy",
                "tidy-fix",
                "vuln"
            ]
        );
        assert_eq!(config.modules, Modules::Auto);
    }

    #[test]
    fn omits_the_release_block_when_not_opted_in() {
        let fragment = render(&detection(), &Answers::new()).expect("render");
        assert!(
            !fragment.table.contains_key("release"),
            "no release block is authored unless the user opts in",
        );
        assert!(parse(&fragment).common.release.is_default());
    }

    #[test]
    fn opting_in_authors_a_tag_only_release_block() {
        let answers = Answers::new()
            .with(toven_ports::RELEASE_ENABLED, Answer::Bool(true))
            .with(toven_ports::RELEASE_HOST, Answer::Bool(true));
        let config = render_with(&answers);

        assert!(
            config.common.release.registry.is_none(),
            "go module tags are registry-less",
        );
        assert_eq!(
            config
                .common
                .release
                .readiness
                .as_deref()
                .expect("readiness authored"),
            ["clean-tree"],
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
            .validate("ecosystems.go.release")
            .expect("authored release validates");
    }

    #[test]
    fn authors_a_coverage_task_and_starter_block() {
        let config = render_with(&Answers::new());
        let coverage = config.common.tasks.get("coverage").expect("coverage task");
        assert_eq!(coverage.argv[..2], ["go", "test"]);
        assert!(
            coverage
                .argv
                .iter()
                .any(|arg| arg == "-coverprofile=target/toven/coverage/go-{module.name}.out")
        );
        assert_eq!(coverage.fan_out, FanOut::WholeWorkspace);
        assert!(!coverage.cacheable, "coverage re-measures every run");
        assert_eq!(coverage.resolved_kind("coverage"), TaskKind::Coverage);
        // Go measures line coverage only: the starter seeds line/changed_line, never
        // function/region, under advisory enforcement.
        assert_eq!(config.common.coverage.line, Some(80.0));
        assert_eq!(config.common.coverage.changed_line, Some(85.0));
        assert_eq!(config.common.coverage.function, None);
        assert_eq!(config.common.coverage.region, None);
        assert_eq!(
            config.common.coverage.enforcement,
            Some(toven_ports::Enforcement::Advisory)
        );
        config
            .common
            .coverage
            .validate("ecosystems.go.coverage")
            .expect("starter coverage validates");
    }

    #[test]
    fn golangci_answer_authors_a_distinct_lint_task() {
        let answers =
            Answers::new().with(LINT_BACKEND, Answer::Choice(ChoiceId::new("golangci-lint")));
        let config = render_with(&answers);
        let lint = config.common.tasks.get("lint").expect("lint task");
        assert_eq!(lint.argv[..2], ["golangci-lint", "run"]);
        assert_eq!(lint.selector, ["./{module.root}/..."]);
        // check and lint do not collide: distinct programs → distinct keys.
        let check = config.common.tasks.get("check").expect("check task");
        assert_ne!(check.argv, lint.argv);
    }

    #[test]
    fn staticcheck_answer_authors_a_staticcheck_lint_task() {
        let answers =
            Answers::new().with(LINT_BACKEND, Answer::Choice(ChoiceId::new("staticcheck")));
        let config = render_with(&answers);
        let lint = config.common.tasks.get("lint").expect("lint task");
        assert_eq!(lint.argv[0], "staticcheck");
    }

    #[test]
    fn vet_lint_answer_omits_the_lint_task() {
        let answers = Answers::new().with(LINT_BACKEND, Answer::Choice(ChoiceId::new("vet")));
        let config = render_with(&answers);
        assert!(!config.common.tasks.contains_key("lint"));
        assert_eq!(
            config.common.tasks.get("check").expect("check").argv[3],
            "vet"
        );
    }

    #[test]
    fn format_is_mutating_and_uncacheable_while_format_check_gates() {
        let config = render_with(&Answers::new());
        let format = config.common.tasks.get("format").expect("format task");
        assert_eq!(format.argv[..2], ["gofmt", "-w"]);
        assert!(!format.cacheable, "the mutating format twin is uncacheable");
        assert_eq!(format.fan_out, FanOut::WholeWorkspace);
        assert_eq!(format.resolved_kind("format"), TaskKind::Format);

        let check = config
            .common
            .tasks
            .get("format-check")
            .expect("format-check task");
        assert_eq!(check.argv[..2], ["gofmt", "-l"]);
        assert!(check.cacheable, "the non-mutating check caches");
        assert!(
            check.fail_if_output,
            "the check gates on the offender list gofmt -l prints"
        );
        assert!(
            !format.fail_if_output,
            "the mutating twin rewrites in place and never gates on output"
        );
        assert_eq!(check.resolved_kind("format-check"), TaskKind::Format);
    }

    #[test]
    fn gofumpt_answer_swaps_the_formatter_program() {
        let answers = Answers::new().with(FORMATTER, Answer::Choice(ChoiceId::new("gofumpt")));
        let config = render_with(&answers);
        assert_eq!(
            config.common.tasks.get("format").expect("format").argv[0],
            "gofumpt"
        );
        assert_eq!(
            config.common.tasks.get("format-check").expect("check").argv[0],
            "gofumpt"
        );
    }

    #[test]
    fn tidy_gates_with_diff_and_tidy_fix_mutates_uncacheably() {
        let config = render_with(&Answers::new());
        let tidy = config.common.tasks.get("tidy").expect("tidy task");
        assert!(tidy.argv.contains(&"-diff".to_string()));
        assert!(tidy.cacheable);
        let fix = config.common.tasks.get("tidy-fix").expect("tidy-fix task");
        assert!(!fix.argv.contains(&"-diff".to_string()));
        assert!(!fix.cacheable, "the mutating tidy twin is uncacheable");
    }

    #[test]
    fn vuln_task_scans_per_module() {
        let config = render_with(&Answers::new());
        let vuln = config.common.tasks.get("vuln").expect("vuln task");
        assert_eq!(vuln.argv[0], "govulncheck");
        assert_eq!(vuln.selector, ["./..."]);
    }

    #[test]
    fn test_hardening_adds_race_and_shuffle() {
        let answers = Answers::new().with(TEST_HARDENING, Answer::Bool(true));
        let config = render_with(&answers);
        let test = config.common.tasks.get("test").expect("test task");
        assert!(test.argv.contains(&"-race".to_string()));
        assert!(test.argv.contains(&"-shuffle=on".to_string()));
    }

    #[test]
    fn default_test_task_is_plain_go_test() {
        let config = render_with(&Answers::new());
        let test = config.common.tasks.get("test").expect("test task");
        assert_eq!(test.argv[..4], ["go", "-C", "{module.root}", "test"]);
        assert!(!test.argv.contains(&"-race".to_string()));
    }

    #[test]
    fn gotestsum_answer_authors_a_gotestsum_test_task() {
        let answers =
            Answers::new().with(TEST_RUNNER, Answer::Choice(ChoiceId::new(RUNNER_GOTESTSUM)));
        let config = render_with(&answers);
        let test = config.common.tasks.get("test").expect("test task");
        assert_eq!(test.argv[0], "gotestsum");
    }

    #[test]
    fn run_task_is_persistent_with_a_bare_run_dot() {
        let config = render_with(&Answers::new());
        let run = config.common.tasks.get("run").expect("run task");
        assert!(run.persistent);
        assert_eq!(
            run.argv,
            ["go", "-C", "{module.root}", "run", ".", "{args}"]
        );
    }

    #[test]
    fn rendered_table_round_trips_through_materialize() {
        let answers =
            Answers::new().with(LINT_BACKEND, Answer::Choice(ChoiceId::new("golangci-lint")));
        let config = render_with(&answers);
        for (key, entry) in &config.common.tasks {
            entry
                .materialize("go", key)
                .expect("every authored entry materializes");
        }
    }
}
