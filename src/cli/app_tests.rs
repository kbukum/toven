use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as StdCommand, ExitCode},
};

use super::{command, run_from, run_with_io};

#[test]
fn help_contains_project_summary() {
    let mut command = command();
    let mut help = Vec::new();
    command.write_long_help(&mut help).expect("help renders");
    let help = String::from_utf8(help).expect("help is utf-8");

    assert!(help.contains("Fast, argv-first development and CI task planning"));
}

#[test]
fn help_subcommand_is_not_treated_as_task() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_with_io(["toven", "help"], &mut stdout, &mut stderr);

    assert_eq!(code, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(
        String::from_utf8(stdout)
            .expect("stdout is utf-8")
            .contains("Fast, argv-first development and CI task planning")
    );
}

#[test]
fn accepts_empty_invocation() {
    command()
        .try_get_matches_from(["toven"])
        .expect("empty invocation parses");
    assert_eq!(run_from(["toven"]), ExitCode::SUCCESS);
}

#[test]
fn reports_usage_errors() {
    assert_eq!(run_from(["toven", "--unknown"]), ExitCode::from(2));
}

#[test]
fn plan_accepts_hyphen_prefixed_passthrough_args() {
    command()
        .try_get_matches_from([
            "toven",
            "plan",
            "--config",
            "toven.toml",
            "--task",
            "test",
            "--",
            "--release",
            "--",
            "next",
        ])
        .expect("plan invocation parses");
}

#[test]
fn plan_accepts_affected_options() {
    command()
        .try_get_matches_from([
            "toven",
            "plan",
            "--affected",
            "--base",
            "origin/main",
            "--merge-base",
        ])
        .expect("affected plan invocation parses");
}

#[test]
fn affected_command_accepts_baseline_options_without_affected_toggle() {
    command()
        .try_get_matches_from([
            "toven",
            "affected",
            "--task",
            "lint",
            "--base",
            "origin/main",
            "--merge-base",
        ])
        .expect("affected invocation parses");

    command()
        .try_get_matches_from(["toven", "affected", "--affected"])
        .expect_err("affected command rejects the run/plan --affected toggle");
}

#[test]
fn run_rejects_conflicting_cache_flags() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with_io(
        ["toven", "smoke", "--no-cache", "--force"],
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(code, ExitCode::from(2));
    assert!(stdout.is_empty());
    let stderr = String::from_utf8(stderr).expect("stderr is utf-8");
    assert!(stderr.contains("--no-cache"));
    assert!(stderr.contains("--force"));
}

#[test]
fn explain_rejects_affected_toggle_and_conflicting_cache_flags() {
    command()
        .try_get_matches_from(["toven", "explain", "fixture-core", "test", "--affected"])
        .expect_err("explain rejects the run/plan --affected toggle");

    command()
        .try_get_matches_from([
            "toven",
            "explain",
            "fixture-core",
            "test",
            "--no-cache",
            "--force",
        ])
        .expect_err("explain rejects conflicting cache flags");
}

#[test]
fn developer_workflow_subcommands_parse() {
    command()
        .try_get_matches_from(["toven", "run", "modules"])
        .expect("explicit run invocation parses reserved task names");
    command()
        .try_get_matches_from(["toven", "modules", "--task", "test"])
        .expect("modules invocation parses");
    command()
        .try_get_matches_from(["toven", "list", "--task", "test"])
        .expect("list alias invocation parses");
    command()
        .try_get_matches_from(["toven", "ls", "--task", "test"])
        .expect("ls alias invocation parses");
    command()
        .try_get_matches_from(["toven", "graph", "--format", "dot"])
        .expect("graph invocation parses");
    command()
        .try_get_matches_from(["toven", "deps", "--format", "dot"])
        .expect("deps alias invocation parses");
    command()
        .try_get_matches_from(["toven", "cache", "stats"])
        .expect("cache stats invocation parses");
    command()
        .try_get_matches_from(["toven", "cache", "info"])
        .expect("cache info alias invocation parses");
    command()
        .try_get_matches_from(["toven", "cache", "clean"])
        .expect("cache clean invocation parses");
    command()
        .try_get_matches_from(["toven", "cache", "clear"])
        .expect("cache clear alias invocation parses");
    command()
        .try_get_matches_from([
            "toven",
            "run",
            "test",
            "--watch",
            "--watch-debounce-ms",
            "50",
        ])
        .expect("watch invocation parses");
}

#[test]
fn run_command_uses_cache_and_reruns_changed_affected_modules() {
    let root = rskit_testutil::test_workspace!("cli-run-cache");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    root.copy_fixture("run-cache/.gitignore", "rust-workspace/.gitignore")
        .expect("copy run-cache gitignore fixture");
    let config_path = write_run_config(&root, &workspace_path);
    init_git_repo(&workspace_path);

    let first = run_cli([
        "toven".to_string(),
        "smoke".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(first.0, ExitCode::SUCCESS, "stderr:\n{}", first.2);
    assert!(first.2.is_empty());
    assert!(first.1.contains("executed"));
    assert_eq!(run_count(&workspace_path), 2);

    let second = run_cli([
        "toven".to_string(),
        "smoke".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(second.0, ExitCode::SUCCESS, "stderr:\n{}", second.2);
    assert!(second.2.is_empty());
    assert!(second.1.contains("cache hit: fixture-core smoke"));
    assert!(!second.1.contains("executed"));
    assert_eq!(run_count(&workspace_path), 2);

    fs::write(
        workspace_path.join("crates/core/src/lib.rs"),
        "pub fn core() -> &'static str { \"changed\" }\n",
    )
    .expect("write changed source");
    let affected = run_cli([
        "toven".to_string(),
        "smoke".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
        "--affected".to_string(),
        "--base".to_string(),
        "HEAD".to_string(),
    ]);
    assert_eq!(affected.0, ExitCode::SUCCESS, "stderr:\n{}", affected.2);
    assert!(affected.2.is_empty());
    assert!(affected.1.contains("executed"));
    assert_eq!(run_count(&workspace_path), 4);
}

#[test]
fn run_command_can_cache_args_when_task_allows_it() {
    let root = rskit_testutil::test_workspace!("cli-run-cache-args");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    root.copy_fixture("run-cache/.gitignore", "rust-workspace/.gitignore")
        .expect("copy run-cache gitignore fixture");
    let config_path = write_run_config_from_template(
        &root,
        &workspace_path,
        "run-cache/toven-cache-args.toml.template",
    );
    init_git_repo(&workspace_path);

    let first = run_smoke_with_args(&config_path, ["--release"]);
    assert_eq!(first.0, ExitCode::SUCCESS, "stderr:\n{}", first.2);
    assert!(first.1.contains("executed"));
    assert_eq!(run_count(&workspace_path), 2);

    let second = run_smoke_with_args(&config_path, ["--release"]);
    assert_eq!(second.0, ExitCode::SUCCESS, "stderr:\n{}", second.2);
    assert!(second.1.contains("cache hit: fixture-core smoke"));
    assert!(!second.1.contains("executed"));
    assert_eq!(run_count(&workspace_path), 2);

    let changed_args = run_smoke_with_args(&config_path, ["--debug"]);
    assert_eq!(
        changed_args.0,
        ExitCode::SUCCESS,
        "stderr:\n{}",
        changed_args.2
    );
    assert!(changed_args.1.contains("executed"));
    assert!(!changed_args.1.contains("cache hit: fixture-core smoke"));
    assert_eq!(run_count(&workspace_path), 4);

    let repeated_changed_args = run_smoke_with_args(&config_path, ["--debug"]);
    assert_eq!(
        repeated_changed_args.0,
        ExitCode::SUCCESS,
        "stderr:\n{}",
        repeated_changed_args.2
    );
    assert!(
        repeated_changed_args
            .1
            .contains("cache hit: fixture-core smoke")
    );
    assert!(!repeated_changed_args.1.contains("executed"));
    assert_eq!(run_count(&workspace_path), 4);
}

#[test]
fn run_command_jsonl_keeps_stdout_machine_readable() {
    let root = rskit_testutil::test_workspace!("cli-run-jsonl");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    root.copy_fixture("run-cache/.gitignore", "rust-workspace/.gitignore")
        .expect("copy run-cache gitignore fixture");
    let config_path = write_run_config(&root, &workspace_path);
    init_git_repo(&workspace_path);

    let output = run_cli([
        "toven".to_string(),
        "smoke".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
        "--output".to_string(),
        "jsonl".to_string(),
    ]);

    assert_eq!(output.0, ExitCode::SUCCESS, "stderr:\n{}", output.2);
    assert!(output.2.contains("executed"));
    let events = output
        .1
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid JSONL"))
        .collect::<Vec<_>>();
    assert!(events.iter().any(|event| event["event"] == "plan.prepared"));
    assert!(events.iter().any(|event| event["event"] == "plan.unit"));
    assert!(events.iter().any(|event| event["event"] == "unit.started"));
    assert!(events.iter().any(|event| event["event"] == "run.summary"));
}

#[test]
fn modules_and_graph_commands_render_discovered_workspace() {
    let root = rskit_testutil::test_workspace!("cli-modules-graph");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    let config_path = workspace_path.join("toven.toml");

    let modules = run_cli([
        "toven".to_string(),
        "modules".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(modules.0, ExitCode::SUCCESS, "stderr:\n{}", modules.2);
    assert!(modules.1.contains("- rust/fixture-core"));
    assert!(modules.1.contains("dependencies: fixture-core"));

    let list_alias = run_cli([
        "toven".to_string(),
        "ls".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(list_alias.0, ExitCode::SUCCESS, "stderr:\n{}", list_alias.2);
    assert!(list_alias.1.contains("- rust/fixture-core"));

    let graph = run_cli([
        "toven".to_string(),
        "graph".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
        "--format".to_string(),
        "dot".to_string(),
    ]);
    assert_eq!(graph.0, ExitCode::SUCCESS, "stderr:\n{}", graph.2);
    assert!(graph.1.contains("digraph toven"));
    assert!(
        graph
            .1
            .contains("\"rust/fixture-app\" -> \"rust/fixture-core\"")
    );
}

#[test]
fn cache_stats_and_clean_report_local_cache_directory() {
    let root = rskit_testutil::test_workspace!("cli-cache-commands");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    let config_path = workspace_path.join("toven.toml");
    let cache_file = workspace_path.join(".toven/cache/v3/aa/record");
    fs::create_dir_all(cache_file.parent().expect("cache parent")).expect("create cache dir");
    fs::write(&cache_file, "cache-record").expect("write cache record");

    let stats = run_cli([
        "toven".to_string(),
        "cache".to_string(),
        "stats".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(stats.0, ExitCode::SUCCESS, "stderr:\n{}", stats.2);
    assert!(stats.1.contains("entries: 1"));
    assert!(stats.1.contains("oldest_age_seconds:"));
    assert!(stats.1.contains("newest_age_seconds:"));
    assert!(stats.1.contains("hit_rate: per-run only"));

    let info = run_cli([
        "toven".to_string(),
        "cache".to_string(),
        "info".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(info.0, ExitCode::SUCCESS, "stderr:\n{}", info.2);
    assert!(info.1.contains("entries: 1"));

    let clean = run_cli([
        "toven".to_string(),
        "cache".to_string(),
        "clean".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);
    assert_eq!(clean.0, ExitCode::SUCCESS, "stderr:\n{}", clean.2);
    assert!(!cache_file.exists());
}

#[cfg(unix)]
#[test]
fn cache_stats_does_not_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let root = rskit_testutil::test_workspace!("cli-cache-symlink");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    let config_path = workspace_path.join("toven.toml");
    let cache_dir = workspace_path.join(".toven/cache/v3");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    fs::write(cache_dir.join("record"), "cache-record").expect("write cache record");
    fs::write(workspace_path.join("outside-cache"), "external").expect("write external file");
    symlink(
        workspace_path.join("outside-cache"),
        cache_dir.join("outside-cache-link"),
    )
    .expect("create cache symlink");

    let stats = run_cli([
        "toven".to_string(),
        "cache".to_string(),
        "stats".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
    ]);

    assert_eq!(stats.0, ExitCode::SUCCESS, "stderr:\n{}", stats.2);
    assert!(stats.1.contains("entries: 1"));
}

#[test]
fn explain_command_reports_affected_and_cache_reasoning() {
    let root = rskit_testutil::test_workspace!("cli-explain");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    init_git_repo(&workspace_path);
    let config_path = workspace_path.join("toven.toml");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with_io(
        [
            "toven".to_string(),
            "explain".to_string(),
            "fixture-core".to_string(),
            "test".to_string(),
            "--config".to_string(),
            config_path.display().to_string(),
            "--base".to_string(),
            "HEAD".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(code, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    let stdout = String::from_utf8(stdout).expect("stdout is utf-8");
    assert!(stdout.contains("module: fixture-core"));
    assert!(stdout.contains("affected: yes (global)"));
    assert!(stdout.contains("cache: miss"));
}

#[test]
fn plan_rejects_baseline_flags_without_affected() {
    let root = rskit_testutil::test_workspace!("cli-plan-base-without-affected");
    let workspace_path = root.path().join("rust-workspace");
    copy_fixture_tree(&root, "rust-workspace", &workspace_path);
    let config_path = workspace_path.join("toven.toml");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with_io(
        [
            "toven".to_string(),
            "plan".to_string(),
            "--config".to_string(),
            config_path.display().to_string(),
            "--base".to_string(),
            "main".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(code, ExitCode::FAILURE);
    assert!(stdout.is_empty());
    let stderr = String::from_utf8(stderr).expect("stderr is utf-8");
    assert!(stderr.contains("--base can only be used with --affected"));
}

fn run_cli<const N: usize>(args: [String; N]) -> (ExitCode, String, String) {
    run_cli_vec(args.into())
}

fn run_smoke_with_args<const N: usize>(
    config_path: &Path,
    passthrough_args: [&str; N],
) -> (ExitCode, String, String) {
    let mut args = vec![
        "toven".to_string(),
        "smoke".to_string(),
        "--config".to_string(),
        config_path.display().to_string(),
        "--".to_string(),
    ];
    args.extend(passthrough_args.into_iter().map(ToString::to_string));
    run_cli_vec(args)
}

fn run_cli_vec(args: Vec<String>) -> (ExitCode, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_with_io(args, &mut stdout, &mut stderr);
    (
        code,
        String::from_utf8(stdout).expect("stdout is utf-8"),
        String::from_utf8(stderr).expect("stderr is utf-8"),
    )
}

fn write_run_config(root: &rskit_testutil::TestWorkspace, workspace: &Path) -> PathBuf {
    write_run_config_from_template(root, workspace, "run-cache/toven.toml.template")
}

fn write_run_config_from_template(
    root: &rskit_testutil::TestWorkspace,
    workspace: &Path,
    template_fixture: &str,
) -> PathBuf {
    let config = workspace.join("toven-run.toml");
    let template = root
        .read_fixture_string(template_fixture)
        .expect("read run-cache config template fixture");
    fs::write(
        &config,
        template.replace("__WORKSPACE_ROOT__", &workspace.display().to_string()),
    )
    .expect("write run config");
    config
}

fn copy_fixture_tree(root: &rskit_testutil::TestWorkspace, fixture: &str, destination: &Path) {
    rskit_fs::sync_io::tree::copy_tree(
        &root.fixture_path(fixture).expect("fixture path"),
        destination,
        rskit_fs::sync_io::tree::CopyTreeOptions::default(),
    )
    .expect("copy fixture tree");
}

fn init_git_repo(workspace: &Path) {
    git(workspace, ["init", "--initial-branch=main", "--quiet"]);
    git(workspace, ["config", "user.name", "Toven Test"]);
    git(workspace, ["config", "user.email", "toven@example.invalid"]);
    git(workspace, ["add", "-A"]);
    git(workspace, ["commit", "--quiet", "-m", "baseline"]);
}

fn git<const N: usize>(workspace: &Path, args: [&str; N]) {
    let output = StdCommand::new("git")
        .current_dir(workspace)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_count(workspace: &Path) -> usize {
    fs::read_to_string(workspace.join(".toven-run-count"))
        .expect("read run count")
        .lines()
        .count()
}
