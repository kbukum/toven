//! Behavior of the scenario engine: materialize → git-script → run → match →
//! effects, with skip and bless flows.
//!
//! The binary under test is a tiny deterministic shell script (the engine is
//! binary-agnostic); scenario data lives under `fixtures/scenarios/engine/`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use rskit_fs::TempDir;
use rskit_fs::sync_io::tree::{CopyTreeOptions, copy_tree};
use rskit_testutil::GoldenMode;
use toven_testkit::SampleRepo;
use toven_testkit::fixtures::scenario_path;
use toven_testkit::git::GitScenario;
use toven_testkit::scenario::{
    GitCommit, GitScript, GitTag, Report, StepStatus, apply_git_script, discover_scenarios,
    run_scenario_with,
};

/// A deterministic stand-in for the `toven` binary: behavior keyed by argv[0],
/// state via the scenario-scoped cache dir and the repo-root cwd.
const FAKE_TOVEN: &str = r#"#!/bin/sh
mode="$1"
case "$mode" in
  cold)
    mkdir -p "$TOVEN_CACHE_DIR"
    printf 'entry\n' > "$TOVEN_CACHE_DIR/e1"
    printf 'cold\n' > out.txt
    printf 'cold run\n'
    ;;
  warm)
    if [ -f "$TOVEN_CACHE_DIR/e1" ]; then printf 'warm run\n'; else printf 'no cache\n'; fi
    ;;
  args)
    printf 'arg:%s\n' "$@"
    ;;
  fail)
    printf 'boom\n' >&2
    exit 3
    ;;
esac
"#;

fn fake_toven(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("fake-toven");
    fs::write(&path, FAKE_TOVEN).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn run_fixture(name: &str, mode: GoldenMode) -> Report {
    let dir = TempDir::new().unwrap();
    let binary = fake_toven(&dir);
    run_scenario_with(
        &binary,
        &scenario_path(name).expect("scenario fixture exists"),
        mode,
        |_| true,
    )
    .unwrap()
}

fn step_statuses(report: &Report) -> Vec<(&str, &StepStatus)> {
    let Report::Completed { steps } = report else {
        panic!("expected a completed report, got {report:?}");
    };
    steps
        .iter()
        .map(|step| (step.id.as_str(), &step.status))
        .collect()
}

#[test]
fn session_runs_ordered_steps_with_goldens_and_effects() {
    let report = run_fixture("engine/session", GoldenMode::Verify);

    let steps = step_statuses(&report);
    assert_eq!(
        steps.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ["01-cold", "02-warm", "03-fail"],
        "steps run in scenario order"
    );
    for (id, status) in steps {
        assert!(
            matches!(status, StepStatus::Passed),
            "step {id} must pass: {status:?}"
        );
    }
}

#[test]
fn golden_mismatch_names_step_and_stream() {
    let report = run_fixture("engine/mismatch", GoldenMode::Verify);

    let steps = step_statuses(&report);
    let (id, status) = steps.last().expect("one step ran");
    assert_eq!(*id, "01-echo");
    let StepStatus::Failed { message } = status else {
        panic!("expected a failed step, got {status:?}");
    };
    assert!(message.contains("01-echo"), "names the step: {message}");
    assert!(message.contains("stdout"), "names the stream: {message}");
}

#[test]
fn exit_code_drift_names_step_and_exit() {
    let report = run_fixture("engine/wrong-exit", GoldenMode::Verify);

    let steps = step_statuses(&report);
    let StepStatus::Failed { message } = steps[0].1 else {
        panic!("expected a failed step, got {:?}", steps[0].1);
    };
    assert!(message.contains("01-boom"), "names the step: {message}");
    assert!(message.contains("exit"), "names the drift: {message}");
}

#[test]
fn effect_failure_names_step_and_effect() {
    let report = run_fixture("engine/bad-effect", GoldenMode::Verify);

    let steps = step_statuses(&report);
    let StepStatus::Failed { message } = steps[0].1 else {
        panic!("expected a failed step, got {:?}", steps[0].1);
    };
    assert!(message.contains("01-noop"), "names the step: {message}");
    assert!(message.contains("nope.txt"), "names the effect: {message}");
}

#[test]
fn argv_is_passed_verbatim_and_config_injected_only_when_set() {
    // The exact goldens embed every argument (including one with a space) on
    // its own line, so a pass proves nothing was rewritten, split, or added —
    // and that `--config` appears only for the step that declares a variant.
    let report = run_fixture("engine/argv-verbatim", GoldenMode::Verify);

    for (id, status) in step_statuses(&report) {
        assert!(
            matches!(status, StepStatus::Passed),
            "step {id} must pass: {status:?}"
        );
    }
}

#[test]
fn missing_toolchain_skips_green() {
    let dir = TempDir::new().unwrap();
    let binary = fake_toven(&dir);
    let scenario = scenario_path("engine/requires-cargo").unwrap();

    let report = run_scenario_with(&binary, &scenario, GoldenMode::Verify, |_| false).unwrap();
    let Report::Skipped { tool } = report else {
        panic!("expected a skipped report, got {report:?}");
    };
    assert_eq!(tool, "cargo");

    // With the toolchain present the same scenario runs.
    let report = run_scenario_with(&binary, &scenario, GoldenMode::Verify, |tool| {
        tool == "cargo"
    })
    .unwrap();
    assert!(matches!(report, Report::Completed { .. }));
}

#[test]
fn step_gate_skips_only_the_gated_step() {
    let dir = TempDir::new().unwrap();
    let binary = fake_toven(&dir);
    let scenario = scenario_path("engine/step-requires").unwrap();

    let report = run_scenario_with(&binary, &scenario, GoldenMode::Verify, |tool| {
        tool != "cargo-cyclonedx"
    })
    .unwrap();

    let steps = step_statuses(&report);
    assert_eq!(
        steps.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ["01-before", "02-gated", "03-after"],
        "a skipped step does not stop the session"
    );
    assert!(matches!(steps[0].1, StepStatus::Passed));
    let StepStatus::Skipped { tool } = steps[1].1 else {
        panic!("expected a skipped step, got {:?}", steps[1].1);
    };
    assert_eq!(tool, "cargo-cyclonedx");
    assert!(
        matches!(steps[2].1, StepStatus::Passed),
        "later steps still run: {:?}",
        steps[2].1
    );
    assert!(report.is_green());
}

#[test]
fn bless_regenerates_goldens_then_verify_passes() {
    let source = scenario_path("engine/session").unwrap();
    let workdir = TempDir::new().unwrap();
    let scenario_dir = workdir.path().join("session");
    copy_tree(&source, &scenario_dir, CopyTreeOptions::default()).unwrap();
    for golden in [
        "01-cold.stdout",
        "02-warm.stdout",
        "03-fail.stderr",
        "out.golden",
    ] {
        fs::remove_file(scenario_dir.join(golden)).unwrap();
    }
    let binary = fake_toven(&workdir);

    let report = run_scenario_with(&binary, &scenario_dir, GoldenMode::Bless, |_| true).unwrap();
    for (id, status) in step_statuses(&report) {
        assert!(
            matches!(status, StepStatus::Blessed),
            "step {id} must bless: {status:?}"
        );
    }
    for golden in [
        "01-cold.stdout",
        "02-warm.stdout",
        "03-fail.stderr",
        "out.golden",
    ] {
        assert!(
            scenario_dir.join(golden).exists(),
            "bless writes {golden} back"
        );
    }

    let report = run_scenario_with(&binary, &scenario_dir, GoldenMode::Verify, |_| true).unwrap();
    for (id, status) in step_statuses(&report) {
        assert!(
            matches!(status, StepStatus::Passed),
            "step {id} must verify against blessed goldens: {status:?}"
        );
    }
}

#[test]
fn git_script_produces_stable_shas_across_materializations() {
    let script = GitScript {
        commits: vec![GitCommit {
            msg: "change app".to_owned(),
            touch: vec!["src/app.txt".to_owned()],
        }],
        tags: vec![GitTag::head("v1")],
        branches: vec!["feature".to_owned()],
    };
    let import_epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let head_of = |script: &GitScript| {
        let repo = SampleRepo::materialize("edge/no-ecosystem").unwrap();
        let git = GitScenario::init(repo.root()).unwrap();
        let import = git
            .commit_all_pinned("import fixture repo", import_epoch)
            .unwrap();
        apply_git_script(&git, script, &import).unwrap();
        (git.resolve("HEAD").unwrap(), git.resolve("v1").unwrap())
    };

    assert_eq!(
        head_of(&script),
        head_of(&script),
        "pinned identity + dates make scripted history byte-stable"
    );
}

#[test]
fn git_script_pins_tags_to_scripted_commits() {
    let script = GitScript {
        commits: vec![GitCommit {
            msg: "change app".to_owned(),
            touch: vec!["src/app.txt".to_owned()],
        }],
        tags: vec![
            GitTag::head("v-head"),
            GitTag::at("v-import", 0),
            GitTag::at("v-change", 1),
        ],
        branches: vec![],
    };
    let repo = SampleRepo::materialize("edge/no-ecosystem").unwrap();
    let git = GitScenario::init(repo.root()).unwrap();
    let import = git
        .commit_all_pinned(
            "import fixture repo",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        )
        .unwrap();

    apply_git_script(&git, &script, &import).unwrap();

    let head = git.resolve("HEAD").unwrap();
    assert_eq!(git.resolve("v-head").unwrap(), head, "plain tag at HEAD");
    assert_eq!(
        git.resolve("v-change").unwrap(),
        head,
        "at = 1 is the only scripted commit"
    );
    assert_eq!(
        git.resolve("v-import").unwrap(),
        import.to_string(),
        "at = 0 is the import commit"
    );
}

#[test]
fn git_script_rejects_a_tag_pin_beyond_the_scripted_history() {
    let script = GitScript {
        commits: vec![GitCommit {
            msg: "change app".to_owned(),
            touch: vec!["src/app.txt".to_owned()],
        }],
        tags: vec![GitTag::at("v-missing", 7)],
        branches: vec![],
    };
    let repo = SampleRepo::materialize("edge/no-ecosystem").unwrap();
    let git = GitScenario::init(repo.root()).unwrap();
    let import = git
        .commit_all_pinned(
            "import fixture repo",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        )
        .unwrap();

    let err = apply_git_script(&git, &script, &import).unwrap_err();
    assert!(
        err.to_string().contains("v-missing"),
        "names the tag: {err}"
    );
    assert!(err.to_string().contains('7'), "names the index: {err}");
}

#[test]
fn discovery_finds_every_scenario_dir_sorted() {
    let root = TempDir::new().unwrap();
    for dir in ["b/nested", "a"] {
        let scenario_dir = root.path().join(dir);
        fs::create_dir_all(&scenario_dir).unwrap();
        fs::write(scenario_dir.join("scenario.yaml"), "repo: r\n").unwrap();
    }
    fs::write(root.path().join("noise.txt"), "not a scenario\n").unwrap();
    fs::create_dir_all(root.path().join("empty")).unwrap();

    let found = discover_scenarios(root.path()).unwrap();
    assert_eq!(
        found,
        vec![root.path().join("a"), root.path().join("b/nested")],
        "scenario dirs are discovered recursively and sorted"
    );
}

#[test]
fn discovery_of_missing_root_is_not_found() {
    let root = TempDir::new().unwrap();
    let err = discover_scenarios(&root.path().join("absent")).unwrap_err();
    assert!(
        err.is_not_found(),
        "missing golden root must be loud: {err}"
    );
}

#[test]
fn git_script_rejects_traversing_touch_paths() {
    let script = GitScript {
        commits: vec![GitCommit {
            msg: "escape".to_owned(),
            touch: vec!["../escape.txt".to_owned()],
        }],
        tags: vec![],
        branches: vec![],
    };
    let repo = SampleRepo::materialize("edge/no-ecosystem").unwrap();
    let git = GitScenario::init(repo.root()).unwrap();
    let import = git
        .commit_all_pinned(
            "import fixture repo",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        )
        .unwrap();

    let err = apply_git_script(&git, &script, &import).unwrap_err();
    assert!(err.to_string().contains("touch"), "names the field: {err}");
}
