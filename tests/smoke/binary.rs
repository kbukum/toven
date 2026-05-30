use std::path::Path;
use std::process::Command;

use crate::case::{ResolvedInvocation, SmokeCommand};

pub struct TovenOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_toven(config: &Path, invocation: &ResolvedInvocation) -> TovenOutput {
    let mut command = Command::new(env!("CARGO_BIN_EXE_toven"));

    match invocation.command {
        SmokeCommand::Plan => {
            command.arg("plan");
            add_config_and_plan_args(&mut command, config, invocation);
        }
        SmokeCommand::Affected => {
            command.arg("affected");
            add_config_and_plan_args(&mut command, config, invocation);
        }
        SmokeCommand::Run => {
            command.arg(
                invocation
                    .task
                    .as_deref()
                    .expect("run invocation has a task"),
            );
            command.arg("--config").arg(config);
            if invocation.no_cache {
                command.arg("--no-cache");
            }
            if invocation.force {
                command.arg("--force");
            }
            if !invocation.args.is_empty() {
                command.arg("--").args(&invocation.args);
            }
        }
    }

    let output = command.output().expect("run toven");
    let status_code = output.status.code().unwrap_or(-1);
    let smoke_output = TovenOutput {
        status_code,
        stdout: String::from_utf8(output.stdout).expect("stdout is utf-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr is utf-8"),
    };

    assert_eq!(
        invocation.expect_status,
        smoke_output.status_code,
        "unexpected exit status for {}\nstdout:\n{}\nstderr:\n{}",
        invocation.label(),
        smoke_output.stdout,
        smoke_output.stderr
    );

    smoke_output
}

fn add_config_and_plan_args(command: &mut Command, config: &Path, invocation: &ResolvedInvocation) {
    command.arg("--config").arg(config);
    if invocation.affected {
        command.arg("--affected");
    }
    if let Some(base) = &invocation.base {
        command.arg("--base").arg(base);
    }
    if invocation.merge_base.unwrap_or(false) {
        command.arg("--merge-base");
    }
    if !invocation.args.is_empty() {
        command.arg("--").args(&invocation.args);
    }
}
