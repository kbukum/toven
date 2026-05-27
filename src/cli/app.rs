//! Clap application definition.

use std::process::ExitCode;

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
    let _matches = command().get_matches();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::command;

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
}
