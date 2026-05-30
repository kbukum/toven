//! Clap application definition and dispatch.

use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

use clap::{Arg, ArgAction, Command};

use crate::cli::{affected::run_affected, explain::run_explain, plan::run_plan, run::run_task};

/// Build the Toven command.
#[must_use]
pub fn command() -> Command {
    Command::new("toven")
        .about("Fast, argv-first development and CI task planning")
        .version(crate::VERSION)
        .subcommand_precedence_over_arg(true)
        .subcommand(plan_command())
        .subcommand(affected_command())
        .subcommand(explain_command())
}

/// Run the CLI.
pub fn run() -> ExitCode {
    run_with_io(std::env::args_os(), &mut io::stdout(), &mut io::stderr())
}

fn run_command() -> Command {
    add_run_args(Command::new("toven"))
}

fn add_run_args(command: Command) -> Command {
    command
        .arg(
            Arg::new("task")
                .value_name("TASK")
                .required(true)
                .help("Task name to execute"),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .default_value("toven.toml")
                .help("Path to the Toven config file"),
        )
        .arg(
            Arg::new("affected")
                .long("affected")
                .action(ArgAction::SetTrue)
                .help("Execute only modules affected by the selected git baseline"),
        )
        .arg(
            Arg::new("base")
                .long("base")
                .value_name("REF")
                .help("Explicit baseline ref or SHA for affected detection"),
        )
        .arg(
            Arg::new("merge-base")
                .long("merge-base")
                .action(ArgAction::SetTrue)
                .help("Use the merge-base of HEAD and the selected baseline ref"),
        )
        .arg(
            Arg::new("no-cache")
                .long("no-cache")
                .action(ArgAction::SetTrue)
                .conflicts_with("force")
                .help("Disable cache reads and writes for execution"),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-cache")
                .help("Skip cache reads but write successful execution records"),
        )
        .arg(
            Arg::new("timeout-seconds")
                .long("timeout-seconds")
                .value_name("SECONDS")
                .value_parser(clap::value_parser!(u64))
                .help("Optional process timeout in seconds"),
        )
        .arg(
            Arg::new("args")
                .num_args(0..)
                .trailing_var_arg(true)
                .allow_hyphen_values(true)
                .help("Arguments passed through to {args}"),
        )
}

fn plan_command() -> Command {
    Command::new("plan")
        .about("Render a reviewable task execution plan")
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .default_value("toven.toml")
                .help("Path to the Toven config file"),
        )
        .arg(
            Arg::new("task")
                .long("task")
                .value_name("NAME")
                .default_value("test")
                .help("Task name to plan"),
        )
        .arg(
            Arg::new("args")
                .num_args(0..)
                .trailing_var_arg(true)
                .allow_hyphen_values(true)
                .help("Arguments passed through to {args}"),
        )
        .arg(
            Arg::new("affected")
                .long("affected")
                .action(ArgAction::SetTrue)
                .help("Plan only modules affected by the selected git baseline"),
        )
        .arg(
            Arg::new("base")
                .long("base")
                .value_name("REF")
                .help("Explicit baseline ref or SHA for affected detection"),
        )
        .arg(
            Arg::new("merge-base")
                .long("merge-base")
                .action(ArgAction::SetTrue)
                .help("Use the merge-base of HEAD and the selected baseline ref"),
        )
}

fn affected_command() -> Command {
    Command::new("affected")
        .about("Show modules affected by a git baseline")
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .default_value("toven.toml")
                .help("Path to the Toven config file"),
        )
        .arg(
            Arg::new("task")
                .long("task")
                .value_name("NAME")
                .default_value("test")
                .help("Task name used to select profiles/modules"),
        )
        .arg(
            Arg::new("base")
                .long("base")
                .value_name("REF")
                .help("Explicit baseline ref or SHA for affected detection"),
        )
        .arg(
            Arg::new("merge-base")
                .long("merge-base")
                .action(ArgAction::SetTrue)
                .help("Use the merge-base of HEAD and the selected baseline ref"),
        )
}

fn explain_command() -> Command {
    Command::new("explain")
        .about("Explain affected and cache reasoning for a module task")
        .arg(
            Arg::new("module")
                .value_name("MODULE")
                .required(true)
                .help("Module name to explain"),
        )
        .arg(
            Arg::new("task")
                .value_name("TASK")
                .required(true)
                .help("Task name to explain"),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .default_value("toven.toml")
                .help("Path to the Toven config file"),
        )
        .arg(
            Arg::new("base")
                .long("base")
                .value_name("REF")
                .help("Explicit baseline ref or SHA for affected detection"),
        )
        .arg(
            Arg::new("merge-base")
                .long("merge-base")
                .action(ArgAction::SetTrue)
                .help("Use the merge-base of HEAD and the selected baseline ref"),
        )
        .arg(
            Arg::new("no-cache")
                .long("no-cache")
                .action(ArgAction::SetTrue)
                .conflicts_with("force")
                .help("Explain with cache disabled"),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-cache")
                .help("Explain with cache reads skipped"),
        )
}

#[cfg(test)]
fn run_from<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_with_io(args, &mut io::sink(), &mut io::sink())
}

fn run_with_io<I, T, Out, Err>(args: I, stdout: &mut Out, stderr: &mut Err) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    Out: Write,
    Err: Write,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<OsString>>();
    if is_task_invocation(&args) {
        return match run_command().try_get_matches_from(args) {
            Ok(matches) => match run_task(&matches, stdout, stderr) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = writeln!(stderr, "error: {}", error.message);
                    ExitCode::FAILURE
                }
            },
            Err(error) => write_clap_error(&error, stdout, stderr),
        };
    }

    match command().try_get_matches_from(args) {
        Ok(matches) => match matches.subcommand() {
            Some(("plan", matches)) => match run_plan(matches, stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = writeln!(stderr, "error: {}", error.message);
                    ExitCode::FAILURE
                }
            },
            Some(("affected", matches)) => match run_affected(matches, stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = writeln!(stderr, "error: {}", error.message);
                    ExitCode::FAILURE
                }
            },
            Some(("explain", matches)) => match run_explain(matches, stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = writeln!(stderr, "error: {}", error.message);
                    ExitCode::FAILURE
                }
            },
            _ => ExitCode::SUCCESS,
        },
        Err(error) => write_clap_error(&error, stdout, stderr),
    }
}

fn is_task_invocation(args: &[OsString]) -> bool {
    let Some(candidate) = args.get(1).and_then(|arg| arg.to_str()) else {
        return false;
    };
    !candidate.starts_with('-') && !matches!(candidate, "help" | "plan" | "affected" | "explain")
}

fn write_clap_error<Out, Err>(error: &clap::Error, stdout: &mut Out, stderr: &mut Err) -> ExitCode
where
    Out: Write,
    Err: Write,
{
    let exit_code = error.exit_code();
    if exit_code == 0 {
        let _ = write!(stdout, "{error}");
    } else {
        let _ = write!(stderr, "{error}");
    }
    ExitCode::from(u8::try_from(exit_code).unwrap_or(1))
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn run_from_accepts_empty_invocation() {
        assert_eq!(run_from(["toven"]), ExitCode::SUCCESS);
    }

    #[test]
    fn run_from_reports_usage_errors() {
        assert_eq!(run_from(["toven", "--unknown"]), ExitCode::from(2));
    }

    #[test]
    fn run_with_io_writes_help_to_stdout() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_io(["toven", "--help"], &mut stdout, &mut stderr);

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let stdout = String::from_utf8(stdout).expect("stdout is utf-8");
        assert!(stdout.contains("Fast, argv-first development and CI task planning"));
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
    fn run_rejects_conflicting_cache_flags() {
        command()
            .try_get_matches_from(["toven", "smoke", "--no-cache", "--force"])
            .expect_err("run rejects conflicting cache flags");
    }

    #[test]
    fn explain_rejects_conflicting_cache_flags() {
        command()
            .try_get_matches_from([
                "toven",
                "explain",
                "fixture-core",
                "smoke",
                "--no-cache",
                "--force",
            ])
            .expect_err("explain rejects conflicting cache flags");
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
        assert!(
            second.1.contains("cache hit: fixture-core smoke"),
            "stdout:\n{}",
            second.1
        );
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
    fn affected_command_accepts_baseline_options() {
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
        assert!(stdout.contains("global_paths:"));
        assert!(stdout.contains("cache: miss"));
        assert!(stdout.contains("source_hash:"));
        assert!(stdout.contains("task_hash:"));
    }

    #[test]
    fn explain_command_reports_unknown_module() {
        let root = rskit_testutil::test_workspace!("cli-explain-unknown-module");
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
                "missing-module".to_string(),
                "test".to_string(),
                "--config".to_string(),
                config_path.display().to_string(),
                "--base".to_string(),
                "HEAD".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(code, ExitCode::FAILURE);
        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).expect("stderr is utf-8");
        assert!(stderr.contains("missing-module"));
    }

    fn run_cli<const N: usize>(args: [String; N]) -> (ExitCode, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_io(args, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).expect("stdout is utf-8"),
            String::from_utf8(stderr).expect("stderr is utf-8"),
        )
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
        let code = super::run_with_io(args, &mut stdout, &mut stderr);
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

    #[test]
    fn plan_command_renders_fixture_workspace() {
        let root = rskit_testutil::test_workspace!("cli-plan");
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
                "--".to_string(),
                "--release".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let stdout = String::from_utf8(stdout).expect("stdout is utf-8");
        assert!(stdout.contains("workspace: fixture"));
        assert!(stdout.contains(r#"argv: ["cargo", "test""#));
        assert!(stdout.contains("--release"));
        assert!(stdout.contains("fixture-core"));
    }

    #[test]
    fn plan_rejects_baseline_flags_without_affected() {
        let root = rskit_testutil::test_workspace!("cli-plan-base-without-affected");
        let workspace_path = root.path().join("rust-workspace");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust-workspace")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");
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
}
