//! Clap application definition and dispatch.

use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

use clap::{Arg, ArgAction, Command};

use crate::cli::{affected::run_affected, plan::run_plan};

/// Build the Toven command.
#[must_use]
pub fn command() -> Command {
    Command::new("toven")
        .about("Fast, argv-first development and CI task planning")
        .version(crate::VERSION)
        .subcommand(plan_command())
        .subcommand(affected_command())
}

/// Run the CLI.
pub fn run() -> ExitCode {
    run_with_io(std::env::args_os(), &mut io::stdout(), &mut io::stderr())
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
            _ => ExitCode::SUCCESS,
        },
        Err(error) => {
            let exit_code = error.exit_code();
            if exit_code == 0 {
                let _ = write!(stdout, "{error}");
            } else {
                let _ = write!(stderr, "{error}");
            }
            ExitCode::from(u8::try_from(exit_code).unwrap_or(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

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
    fn plan_command_renders_fixture_workspace() {
        let root = rskit_testutil::test_workspace!("cli-plan");
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
