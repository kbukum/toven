//! CLI dispatch and exit-code handling.

use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

use crate::cli::{
    affected::run_affected,
    cache::run_cache,
    commands::{RESERVED_SUBCOMMANDS, command, run_command},
    explain::run_explain,
    graph::run_graph,
    modules::run_modules,
    plan::run_plan,
    run::run_task_with_streaming,
};
use crate::generate::run_generate;

/// Run the CLI with process stdio.
pub(super) fn run() -> ExitCode {
    run_with_io_and_streaming(
        std::env::args_os(),
        &mut io::stdout(),
        &mut io::stderr(),
        true,
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

#[cfg(test)]
fn run_with_io<I, T, Out, Err>(args: I, stdout: &mut Out, stderr: &mut Err) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    Out: Write,
    Err: Write,
{
    run_with_io_and_streaming(args, stdout, stderr, false)
}

fn run_with_io_and_streaming<I, T, Out, Err>(
    args: I,
    stdout: &mut Out,
    stderr: &mut Err,
    stream_output: bool,
) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    Out: Write,
    Err: Write,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<OsString>>();
    if is_task_invocation(&args) {
        return match run_command().try_get_matches_from(args) {
            Ok(matches) => exit_from_result(
                run_task_with_streaming(&matches, stdout, stderr, stream_output),
                stderr,
            ),
            Err(error) => write_clap_error(&error, stdout, stderr),
        };
    }

    match command().try_get_matches_from(args) {
        Ok(matches) => match matches.subcommand() {
            Some(("plan", matches)) => exit_from_result(run_plan(matches, stdout), stderr),
            Some(("run", matches)) => exit_from_result(
                run_task_with_streaming(matches, stdout, stderr, stream_output),
                stderr,
            ),
            Some(("affected", matches)) => exit_from_result(run_affected(matches, stdout), stderr),
            Some(("explain", matches)) => exit_from_result(run_explain(matches, stdout), stderr),
            Some(("modules" | "list" | "ls", matches)) => {
                exit_from_result(run_modules(matches, stdout), stderr)
            }
            Some(("graph" | "deps", matches)) => {
                exit_from_result(run_graph(matches, stdout), stderr)
            }
            Some(("cache", matches)) => exit_from_result(run_cache(matches, stdout), stderr),
            Some(("generate", matches)) => exit_from_result(run_generate(matches, stdout), stderr),
            _ => ExitCode::SUCCESS,
        },
        Err(error) => write_clap_error(&error, stdout, stderr),
    }
}

fn is_task_invocation(args: &[OsString]) -> bool {
    let Some(candidate) = args.get(1).and_then(|arg| arg.to_str()) else {
        return false;
    };
    !candidate.starts_with('-') && !RESERVED_SUBCOMMANDS.contains(&candidate)
}

fn exit_from_result(result: crate::core::AppResult<()>, stderr: &mut impl Write) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(stderr, "error: {}", error.message);
            ExitCode::FAILURE
        }
    }
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
#[path = "app_tests.rs"]
mod tests;
