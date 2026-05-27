//! Clap application definition.

use std::{ffi::OsString, process::ExitCode};

use clap::Command;

/// Build the Toven command.
#[must_use]
pub fn command() -> Command {
    Command::new("toven")
        .about("Fast, argv-first development and CI task planning")
        .version(crate::VERSION)
}

/// Run the CLI.
pub fn run() -> ExitCode {
    run_from(std::env::args_os())
}

fn run_from<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match command().try_get_matches_from(args) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = error.print();
            ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{command, run_from};

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
}
