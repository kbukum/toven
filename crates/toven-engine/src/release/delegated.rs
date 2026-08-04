//! [`ProcessDelegatedPhase`] — the concrete [`DelegatedPhase`] runner, backed by
//! the rskit process port.
//!
//! The engine owns selection, ordering, readiness, safety, and reporting; this
//! runner owns exactly one mechanical step — spawn the fully-resolved argument
//! vector, forward the named secrets through the child-process environment, and
//! report the classified exit. It mirrors [`super::verify`]'s `run_tool`: a
//! bounded, captured output and a single shared timeout, argv-only.
//!
//! Secrets never touch argv or logs. A delegated phase names the environment
//! variables its tool may read ([`DelegatedPhaseRequest::forward_env`]); the
//! runner resolves each name from the ambient environment via
//! [`rskit_util::env::get_non_empty`] and sets it on the child, so the value
//! flows to the tool by environment only.

use std::path::PathBuf;
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_process::{CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_model::ReleasePhase;
use toven_ports::{
    DelegatedPhase, DelegatedPhaseMode, DelegatedPhaseOutcome, DelegatedPhaseRequest, DelegatedTool,
};

/// Shared timeout for a delegated-phase invocation, matching the release-verify
/// tool timeout so no delegated tool waits unbounded.
const DELEGATED_TIMEOUT: Duration = Duration::from_mins(5);

/// Upper bound on captured stdout/stderr per delegated invocation, so a chatty
/// tool cannot exhaust memory; the classified outcome carries the bounded
/// output.
const MAX_DELEGATED_OUTPUT_BYTES: usize = 256 * 1024;

/// Build the argv-first [`DelegatedPhaseRequest`] for one delegated phase.
///
/// The mutation posture selects the argument vector: [`DelegatedPhaseMode::Preview`]
/// uses the tool's mandatory mutation-free preview arguments (its dry-run/plan
/// equivalent), while [`DelegatedPhaseMode::Apply`] uses its real, mutating
/// arguments. The tool name is always the first argv element, so the invocation
/// is argv-first with no shell. Secrets never enter argv — `forward_env` names
/// the environment variables the child may read; the runner resolves their
/// values from the ambient environment.
pub fn delegated_request(
    phase: ReleasePhase,
    tool: &DelegatedTool,
    mode: DelegatedPhaseMode,
    working_dir: impl Into<PathBuf>,
    forward_env: Vec<String>,
) -> DelegatedPhaseRequest {
    let mut argv = Vec::new();
    argv.push(tool.tool.clone());
    match mode {
        DelegatedPhaseMode::Apply => {
            if let Some(args) = &tool.args {
                argv.extend(args.iter().cloned());
            }
        }
        // Preview and any future non-mutating posture default to the mandatory
        // mutation-free preview argv, so an unrecognized mode never mutates.
        _ => argv.extend(tool.preview.iter().cloned()),
    }
    DelegatedPhaseRequest::new(phase, argv, mode, working_dir).with_forward_env(forward_env)
}

/// [`DelegatedPhase`] runner backed by [`rskit_process`].
///
/// Stateless and cheap to construct; holds no credentials. Secrets are resolved
/// from the ambient environment at run time by the names the request forwards,
/// never stored on the runner and never placed on argv.
#[derive(Debug, Clone, Default)]
pub struct ProcessDelegatedPhase;

impl ProcessDelegatedPhase {
    /// Construct a process-backed delegated-phase runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DelegatedPhase for ProcessDelegatedPhase {
    fn run(&self, request: &DelegatedPhaseRequest) -> AppResult<DelegatedPhaseOutcome> {
        let (program, args) = request.argv.split_first().ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidInput,
                "delegated phase request has an empty argv",
            )
            .with_detail("phase", request.phase.as_str())
        })?;

        let mut spec = ProcessSpec::new(program)
            .args(args.iter().cloned())
            .dir(&request.working_dir);
        // Forward named secrets by environment only: resolve each name from the
        // ambient environment and set it on the child. A name that is unset or
        // empty is skipped rather than forwarded as blank, and no value is ever
        // placed on argv or logged.
        for name in &request.forward_env {
            if let Some(value) = rskit_util::env::get_non_empty(name) {
                spec = spec.env(name.clone(), value);
            }
        }

        let config = ProcessConfig::default()
            .with_timeout(Some(DELEGATED_TIMEOUT))
            .with_io(ProcessIo::captured(CapturedIo::new().with_output(
                OutputPolicy::captured().with_max_output_bytes(MAX_DELEGATED_OUTPUT_BYTES),
            )));

        let result = run(&spec, &config)?;
        // Timeout and cancellation are not a normal tool exit — surface them as
        // typed errors. A non-zero exit, by contrast, is a valid classified
        // outcome the engine maps against the phase's guarantees.
        if result.timed_out {
            return Err(AppError::new(
                ErrorCode::Timeout,
                format!(
                    "delegated {} tool `{program}` timed out",
                    request.phase.as_str()
                ),
            )
            .with_detail("phase", request.phase.as_str())
            .with_detail("timed_out", true));
        }
        if result.cancelled {
            return Err(AppError::new(
                ErrorCode::Cancelled,
                format!(
                    "delegated {} tool `{program}` was cancelled",
                    request.phase.as_str()
                ),
            )
            .with_detail("phase", request.phase.as_str()));
        }

        Ok(DelegatedPhaseOutcome::new(
            result.exit_code,
            result.stdout,
            result.stderr,
        ))
    }
}

#[cfg(test)]
mod tests {
    use toven_model::ReleasePhase;
    use toven_ports::{DelegatedPhase, DelegatedPhaseMode, DelegatedTool};
    use toven_testkit::FakeDelegatedPhase;

    use super::{ProcessDelegatedPhase, delegated_request};

    fn goreleaser() -> DelegatedTool {
        DelegatedTool {
            tool: "goreleaser".into(),
            args: Some(vec!["release".into(), "--clean".into()]),
            preview: vec!["release".into(), "--snapshot".into(), "--clean".into()],
        }
    }

    #[test]
    fn preview_mode_builds_the_mutation_free_argv_tool_first() {
        let request = delegated_request(
            ReleasePhase::Package,
            &goreleaser(),
            DelegatedPhaseMode::Preview,
            "/repo",
            vec!["GITHUB_TOKEN".into()],
        );

        assert_eq!(
            request.argv,
            vec![
                "goreleaser".to_string(),
                "release".into(),
                "--snapshot".into(),
                "--clean".into(),
            ]
        );
        assert_eq!(request.tool(), Some("goreleaser"));
        assert_eq!(request.mode, DelegatedPhaseMode::Preview);
        assert_eq!(request.working_dir, std::path::Path::new("/repo"));
    }

    #[test]
    fn apply_mode_builds_the_mutating_argv_tool_first() {
        let request = delegated_request(
            ReleasePhase::Package,
            &goreleaser(),
            DelegatedPhaseMode::Apply,
            "/repo",
            Vec::new(),
        );

        assert_eq!(
            request.argv,
            vec!["goreleaser".to_string(), "release".into(), "--clean".into()]
        );
    }

    #[test]
    fn secrets_are_named_on_forward_env_never_on_argv() {
        let request = delegated_request(
            ReleasePhase::Publish,
            &goreleaser(),
            DelegatedPhaseMode::Preview,
            "/repo",
            vec!["GITHUB_TOKEN".into(), "REGISTRY_TOKEN".into()],
        );

        assert_eq!(request.forward_env, vec!["GITHUB_TOKEN", "REGISTRY_TOKEN"]);
        // The secret variable *names* may appear as env keys, but never their
        // values, and no token value is ever placed on argv.
        assert!(
            request.argv.iter().all(|arg| !arg.contains("TOKEN")),
            "argv leaked a secret: {:?}",
            request.argv
        );
    }

    #[test]
    fn the_engine_drives_a_delegated_preview_argv_first_through_the_runner() {
        let runner = FakeDelegatedPhase::new().with_exit_code(Some(0));
        let request = delegated_request(
            ReleasePhase::Package,
            &goreleaser(),
            DelegatedPhaseMode::Preview,
            "/repo",
            vec!["GITHUB_TOKEN".into()],
        );

        let outcome = runner.run(&request).expect("preview runs");

        assert!(outcome.succeeded());
        let recorded = runner.requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].mode, DelegatedPhaseMode::Preview);
        assert_eq!(
            recorded[0].argv.first().map(String::as_str),
            Some("goreleaser")
        );
        assert!(recorded[0].argv.contains(&"--snapshot".to_string()));
    }

    #[test]
    fn an_empty_argv_is_a_typed_error() {
        let runner = ProcessDelegatedPhase::new();
        let request = toven_ports::DelegatedPhaseRequest::new(
            ReleasePhase::Package,
            Vec::new(),
            DelegatedPhaseMode::Preview,
            "/repo",
        );

        let error = runner.run(&request).expect_err("empty argv is rejected");
        assert!(error.to_string().contains("empty argv"), "{error}");
    }
}
