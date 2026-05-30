//! Execution unit rendering and subprocess execution.

pub(crate) mod persistent;
mod render;
mod runner;

use std::path::Path;

use crate::{
    core::{AppResult, ExecutionUnit},
    exec::persistent::PersistentRun,
};

pub use render::{render_execution_unit, render_resource_group};
pub use runner::{RunOptions, RunOutput, run_execution_unit};

#[derive(Clone, Copy)]
pub(crate) struct PersistentOutput {
    stdout: Option<PersistentOutputStream>,
    stderr: Option<PersistentOutputStream>,
}

impl PersistentOutput {
    pub(crate) const fn forward(
        stdout: PersistentOutputStream,
        stderr: PersistentOutputStream,
    ) -> Self {
        Self {
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }

    #[cfg(test)]
    const fn capture_only() -> Self {
        Self {
            stdout: None,
            stderr: None,
        }
    }

    const fn stdout_stream(self) -> Option<PersistentOutputStream> {
        self.stdout
    }

    const fn stderr_stream(self) -> Option<PersistentOutputStream> {
        self.stderr
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PersistentOutputStream {
    Stdout,
    Stderr,
}

pub(crate) struct PersistentProcess(persistent::PersistentProcess);

impl PersistentProcess {
    pub(crate) fn wait(self) -> AppResult<()> {
        self.0.wait()
    }

    pub(crate) fn shutdown(self) -> AppResult<()> {
        self.0.shutdown()
    }
}

pub(crate) fn start_persistent_execution_unit_with_output(
    unit: &ExecutionUnit,
    workspace_root: &Path,
    options: &RunOptions,
    output: PersistentOutput,
) -> AppResult<(RunOutput, PersistentProcess)> {
    let PersistentRun { output, process } =
        persistent::start_persistent_execution_unit_with_output(
            unit,
            workspace_root,
            options,
            output,
        )?;
    Ok((output, PersistentProcess(process)))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::core::{CommandOrigin, ExecutionMode, ExecutionUnit, PersistentReadiness};

    use super::{
        PersistentOutput, PersistentOutputStream, RunOptions,
        start_persistent_execution_unit_with_output,
    };

    #[test]
    fn wrapper_starts_persistent_process() {
        let root = rskit_testutil::test_workspace!("exec-wrapper-persistent");
        let unit = ExecutionUnit {
            id: "dev/server/workspace".to_string(),
            profile: "dev".to_string(),
            task: "server".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            mode: ExecutionMode::WorkspaceOnce,
            resource_group: String::new(),
            modules: Vec::new(),
            argv_template: vec!["sh".to_string(), "-c".to_string(), "sleep 0.01".to_string()],
            module_arg_template: Vec::new(),
            passthrough_args: Vec::new(),
            cache_args: false,
            persistent: true,
            readiness: PersistentReadiness::Started,
            readiness_timeout: Duration::from_secs(1),
            shared_inputs: Vec::new(),
        };

        let (output, process) = start_persistent_execution_unit_with_output(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
            PersistentOutput::forward(
                PersistentOutputStream::Stderr,
                PersistentOutputStream::Stderr,
            ),
        )
        .expect("persistent process starts");

        assert!(output.result.success());
        process.wait().expect("process exits cleanly");
    }
}
