//! [`ProcessToolRunner`] — the concrete synchronous [`ToolRunner`], backed by
//! the rskit process port.
//!
//! Composes `rskit-process`'s blocking [`run`] with Toven's argv-first policy:
//! captured, bounded output; an optional wall-clock timeout; the explicit
//! environment policy; and named-secret forwarding resolved from the ambient
//! environment at run time. It never decides success/failure policy — it maps
//! the process result straight into a [`ToolOutcome`] the caller classifies.

use rskit_errors::{AppError, AppResult};
use rskit_process::{CapturedIo, EnvPolicy, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_ports::{InvocationEnvPolicy, ToolInvocation, ToolOutcome, ToolRunner};

/// The production [`ToolRunner`].
///
/// A captured, bounded, optionally-timed subprocess. Stateless and cheap to
/// construct; holds no credentials — secrets are resolved from the ambient
/// environment at run time by the names the invocation forwards, never stored
/// and never placed on argv.
#[derive(Debug, Clone, Default)]
pub struct ProcessToolRunner;

impl ProcessToolRunner {
    /// Construct a process-backed tool runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ToolRunner for ProcessToolRunner {
    fn run(&self, invocation: &ToolInvocation) -> AppResult<ToolOutcome> {
        let (program, args) = invocation
            .argv
            .split_first()
            .ok_or_else(|| AppError::invalid_input("tool.argv", "must include a program"))?;

        let mut spec = ProcessSpec::new(program)
            .args(args.iter().cloned())
            .env_policy(match invocation.environment.policy {
                InvocationEnvPolicy::ExplicitOnly => EnvPolicy::Empty,
                InvocationEnvPolicy::InheritParent => EnvPolicy::Inherit,
            })
            .envs(invocation.environment.vars.clone());
        if let Some(dir) = invocation.working_dir() {
            spec = spec.dir(dir);
        }
        // Forward named secrets by environment only: resolve each from the
        // ambient environment and set it on the child. A name that is unset or
        // empty is skipped rather than forwarded blank, and no value is ever
        // placed on argv or logged.
        for name in &invocation.forward_env {
            if let Some(value) = rskit_util::env::get_non_empty(name) {
                spec = spec.env(name.clone(), value);
            }
        }

        // `with_timeout(None)` clears rskit-process's inherited 30s default so an
        // invocation without a declared bound runs unbounded; a declared bound is
        // honored. Captured output stays bounded by the runner default cap unless
        // the invocation narrows it further.
        let mut config = ProcessConfig::default()
            .with_io(ProcessIo::captured(CapturedIo::new()))
            .with_timeout(invocation.timeout);
        if let Some(max_output_bytes) = invocation.max_output_bytes {
            config = config.with_max_output_bytes(max_output_bytes);
        }

        let result = run(&spec, &config)?;
        Ok(
            ToolOutcome::new(result.exit_code, result.stdout, result.stderr)
                .timed_out_flag(result.timed_out)
                .cancelled_flag(result.cancelled),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_ports::{InvocationEnvironment, ToolInvocation, ToolRunner};

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
}
