//! Clap application definition and dispatch.

use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

use clap::{Arg, Command};

use crate::cli::plan::run_plan;

/// Build the Toven command.
#[must_use]
pub fn command() -> Command {
    Command::new("toven")
        .about("Fast, argv-first development and CI task planning")
        .version(crate::VERSION)
        .subcommand(plan_command())
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
            _ => ExitCode::SUCCESS,
        },
        Err(error) => {
            let _ = write!(stderr, "{error}");
            ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
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
        assert!(stdout.contains("cargo test"));
        assert!(stdout.contains("--release"));
        assert!(stdout.contains("fixture-core"));
    }
}
