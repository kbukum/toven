//! [`ProcessToolRunner`] — the concrete synchronous [`ToolRunner`], backed by
//! the rskit process port.
//!
//! Composes the shared argv→[`ProcessSpec`](rskit_process::ProcessSpec)
//! lowering ([`spec`](super::spec)) with `rskit-process`'s blocking
//! [`run`](rskit_process::run): captured, bounded output; an optional
//! wall-clock timeout; the explicit environment policy; and named-secret
//! forwarding resolved from the ambient environment at run time. It never
//! decides success/failure policy — it maps the process result straight into a
//! [`ToolOutcome`] the caller classifies.

use std::sync::Arc;

use rskit_errors::AppResult;
use rskit_process::{LifecyclePolicy, ProcessSupervisor, run_supervised};
use toven_ports::{ToolInvocation, ToolOutcome, ToolRunner};

use crate::spec::{tool_config, tool_spec};

/// The production [`ToolRunner`].
///
/// A captured, bounded, optionally-timed subprocess. Cheap to construct and
/// holds no credentials — secrets are resolved from the ambient environment at
/// run time by the names the invocation forwards, never stored and never placed
/// on argv. Each spawned child registers with a [`ProcessSupervisor`] so a
/// process-level shutdown reaps the tool's process group as the backstop.
#[derive(Debug, Clone)]
pub struct ProcessToolRunner {
    supervisor: Arc<ProcessSupervisor>,
}

impl Default for ProcessToolRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessToolRunner {
    /// Construct a process-backed tool runner owning a private supervisor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            supervisor: Arc::new(ProcessSupervisor::new(LifecyclePolicy::default())),
        }
    }

    /// Drive spawned children through a caller-owned [`ProcessSupervisor`] so a
    /// process-level shutdown handle can reap them as the backstop.
    #[must_use]
    pub fn with_supervisor(mut self, supervisor: Arc<ProcessSupervisor>) -> Self {
        self.supervisor = supervisor;
        self
    }

    /// The supervisor this runner registers spawned children with.
    #[must_use]
    pub fn supervisor(&self) -> Arc<ProcessSupervisor> {
        Arc::clone(&self.supervisor)
    }
}

impl ToolRunner for ProcessToolRunner {
    fn run(&self, invocation: &ToolInvocation) -> AppResult<ToolOutcome> {
        let spec = tool_spec(invocation)?;
        let config = tool_config(invocation);
        let result = run_supervised(&self.supervisor, &spec, &config)?;
        Ok(
            ToolOutcome::new(result.exit_code, result.stdout, result.stderr)
                .timed_out_flag(result.timed_out)
                .cancelled_flag(result.cancelled)
                .truncated_flags(result.stdout_truncated, result.stderr_truncated),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_ports::{ForwardEnvAs, InvocationEnvironment, ToolInvocation, ToolRunner};

    use super::ProcessToolRunner;

    #[test]
    fn an_empty_argv_is_a_typed_error() {
        let error = ProcessToolRunner::new()
            .run(&ToolInvocation::new(Vec::new()))
            .expect_err("empty argv is rejected");
        assert!(
            error.to_string().contains("must include a program"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_zero_exit_tool_reports_its_captured_stdout() {
        let outcome = ProcessToolRunner::new()
            .run(&ToolInvocation::new(vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf hello".into(),
            ]))
            .expect("runs");
        assert!(outcome.succeeded());
        assert_eq!(outcome.stdout, "hello");
    }

    #[cfg(unix)]
    #[test]
    fn a_non_zero_exit_is_reported_as_an_outcome_not_an_error() {
        let outcome = ProcessToolRunner::new()
            .run(&ToolInvocation::new(vec![
                "/bin/sh".into(),
                "-c".into(),
                "exit 3".into(),
            ]))
            .expect("a non-zero exit is not a spawn error");
        assert!(!outcome.succeeded());
        assert_eq!(outcome.exit_code, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn the_default_policy_inherits_the_ambient_environment() {
        // Regression: a bare-name tool (`goreleaser`, `cosign`, `gh`) needs the
        // ambient `PATH` to be resolvable and typically `HOME`/VCS config too, so
        // the default invocation must inherit — not clear — the parent
        // environment. A default-empty policy spawns every real delegated/one-shot
        // tool under `env_clear()` and breaks it. Compare against the parent's own
        // `PATH` (not merely "non-empty"): a cleared environment makes POSIX `sh`
        // synthesize a default `PATH`, so only an exact match proves inheritance.
        let parent_path = std::env::var("PATH").expect("the test runner has PATH set");
        assert!(!parent_path.is_empty(), "precondition: parent PATH is set");

        let inherited = ProcessToolRunner::new()
            .run(&ToolInvocation::new(vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf %s \"$PATH\"".into(),
            ]))
            .expect("runs");
        assert_eq!(
            inherited.stdout, parent_path,
            "default policy must inherit the ambient PATH"
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_environment_variables_reach_the_child() {
        let mut vars = BTreeMap::new();
        vars.insert("TOVEN_TOOL_TEST".to_string(), "present".to_string());
        let outcome = ProcessToolRunner::new()
            .run(
                &ToolInvocation::new(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf %s \"$TOVEN_TOOL_TEST\"".into(),
                ])
                .with_environment(InvocationEnvironment::explicit(vars)),
            )
            .expect("runs");
        assert_eq!(outcome.stdout, "present");
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_is_reported_in_the_outcome_flags() {
        let outcome = ProcessToolRunner::new()
            .run(
                &ToolInvocation::new(vec!["/bin/sleep".into(), "5".into()])
                    .with_timeout(std::time::Duration::from_millis(50)),
            )
            .expect("runs");
        assert!(outcome.timed_out);
        // A shared `require_success` turns the timeout into a typed error.
        let error = outcome
            .require_success("sleep")
            .expect_err("timeout fails closed");
        assert_eq!(error.code(), rskit_errors::ErrorCode::Timeout);
    }

    #[cfg(unix)]
    #[test]
    fn piped_stdin_reaches_the_tool() {
        // The stdin lowering (`spec::tool_config`) is exercised end to end: bytes
        // handed to `with_stdin` must arrive on the child's standard input.
        let outcome = ProcessToolRunner::new()
            .run(&ToolInvocation::new(vec!["/bin/cat".into()]).with_stdin(b"piped-notes".to_vec()))
            .expect("runs");
        assert!(outcome.succeeded());
        assert_eq!(outcome.stdout, "piped-notes");
    }

    #[cfg(unix)]
    #[test]
    fn an_overflowing_capture_is_reported_as_truncated_and_fails_closed() {
        // A tool that exits zero but overruns the output bound must not read as a
        // clean success: the truncation flag rides through to the outcome so a
        // consumer of the (now incomplete) output fails closed.
        let outcome = ProcessToolRunner::new()
            .run(
                &ToolInvocation::new(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf 'aaaaaaaaaa'".into(),
                ])
                .with_max_output_bytes(4),
            )
            .expect("runs");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.truncated.stdout);
        assert!(!outcome.succeeded());
        let error = outcome
            .require_success("bounded tool")
            .expect_err("truncated output fails closed");
        assert_eq!(error.code(), rskit_errors::ErrorCode::Internal);
    }

    #[test]
    fn a_missing_renamed_secret_source_fails_closed() {
        // `forward_env_as` is a required rename: a configured source that is
        // unset at run time must be a typed error, never a silent skip that lets
        // a publish proceed with an unintended (or no) credential. The source
        // name is guaranteed absent, so no environment mutation is needed.
        let error = ProcessToolRunner::new()
            .run(
                &ToolInvocation::new(vec!["/bin/echo".into()]).with_forward_env_as(vec![
                    ForwardEnvAs::new(
                        "TOVEN_EXEC_DEFINITELY_UNSET_SECRET_SOURCE",
                        "CARGO_REGISTRY_TOKEN",
                    ),
                ]),
            )
            .expect_err("a missing required secret source fails closed");
        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(error.to_string().contains("unset or empty"), "{error}");
    }
}
