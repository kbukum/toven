//! Delegated-phase execution — building the argv-first [`ToolInvocation`] for a
//! delegated release tool and driving it through the shared [`ToolRunner`] seam.
//!
//! The engine owns selection, ordering, readiness, safety, and reporting; the
//! runner owns exactly one mechanical step — spawn the fully-resolved argument
//! vector, forward the named secrets through the child-process environment, and
//! report the classified exit. It shares the one [`ToolRunner`] seam with
//! `release verify`, hosted-release CLIs, and every other one-shot tool call, so
//! nothing here re-wires `rskit-process` or re-implements exit mapping.
//!
//! Secrets never touch argv or logs. A delegated phase names the environment
//! variables its tool may read; the invocation forwards each name and the runner
//! resolves its non-empty value from the ambient environment, so the value flows
//! to the tool by environment only.

use std::path::PathBuf;
use std::time::Duration;

use rskit_errors::AppResult;
use toven_model::ReleasePhase;
use toven_ports::{DelegatedTool, ToolInvocation, ToolOutcome, ToolRunner};

/// Shared timeout for a delegated-phase invocation, matching the release-verify
/// tool timeout so no delegated tool waits unbounded.
const DELEGATED_TIMEOUT: Duration = Duration::from_mins(5);

/// Upper bound on captured stdout/stderr per delegated invocation, so a chatty
/// tool cannot exhaust memory; the classified outcome carries the bounded
/// output.
const MAX_DELEGATED_OUTPUT_BYTES: usize = 256 * 1024;

/// The mutation posture that selects a delegated tool's argument vector.
///
/// [`Preview`](Self::Preview) uses the tool's mandatory mutation-free preview
/// arguments (its dry-run/plan equivalent); [`Apply`](Self::Apply) uses its
/// real, mutating arguments. The engine chooses the posture; the tool name is
/// always the first argv element, so the invocation is argv-first with no shell.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DelegatedPhaseMode {
    /// Run the tool's mutation-free preview (dry-run/snapshot) arguments.
    Preview,
    /// Run the tool's real, mutating arguments.
    Apply,
}

/// Build the argv-first [`ToolInvocation`] for one delegated phase.
///
/// The mutation posture selects the argument vector: [`DelegatedPhaseMode::Preview`]
/// uses the tool's mandatory mutation-free preview arguments (its dry-run/plan
/// equivalent), while [`DelegatedPhaseMode::Apply`] uses its real, mutating
/// arguments. The tool name is always the first argv element, so the invocation
/// is argv-first with no shell. Secrets never enter argv — `forward_env` names
/// the environment variables the child may read; the runner resolves their
/// values from the ambient environment.
pub fn delegated_request(
    tool: &DelegatedTool,
    mode: DelegatedPhaseMode,
    working_dir: impl Into<PathBuf>,
    forward_env: Vec<String>,
) -> ToolInvocation {
    let mut argv = Vec::new();
    argv.push(tool.tool.clone());
    match mode {
        DelegatedPhaseMode::Apply => {
            if let Some(args) = &tool.args {
                argv.extend(args.iter().cloned());
            }
        }
        DelegatedPhaseMode::Preview => argv.extend(tool.preview.iter().cloned()),
    }
    ToolInvocation::new(argv)
        .with_working_dir(working_dir)
        .with_forward_env(forward_env)
        .with_timeout(DELEGATED_TIMEOUT)
        .with_max_output_bytes(MAX_DELEGATED_OUTPUT_BYTES)
}

/// Drive a delegated phase through the runner as a **mutation-free preview** and
/// fail closed on a non-zero tool exit.
///
/// The non-mutating asset verbs (`release package`, `release sign`) run the
/// delegated tool in [`DelegatedPhaseMode::Preview`] (its `--snapshot`/dry-run
/// equivalent), which produces local artifacts without publishing anything. The
/// engine still owns selection, ordering, and reporting; this only runs the
/// tool and classifies its exit through the shared
/// [`ToolOutcome::require_success`], mapping a non-zero exit into a typed error
/// carrying the tool's captured stderr so the failure is actionable rather than
/// swallowed.
///
/// # Errors
/// Propagates a spawn/IO failure from the runner and converts a non-zero exit,
/// timeout, or signal-kill into a typed error.
pub fn run_delegated_preview(
    phase: ReleasePhase,
    tool: &DelegatedTool,
    runner: &dyn ToolRunner,
    working_dir: impl Into<PathBuf>,
) -> AppResult<()> {
    let invocation = delegated_request(tool, DelegatedPhaseMode::Preview, working_dir, Vec::new());
    let outcome: ToolOutcome = runner.run(&invocation)?;
    outcome.require_success(&format!(
        "delegated {} tool `{}`",
        phase.as_str(),
        tool.tool
    ))
}

#[cfg(test)]
mod tests {
    use toven_model::ReleasePhase;
    use toven_ports::DelegatedTool;
    use toven_testkit::FakeToolRunner;

    use super::{DelegatedPhaseMode, delegated_request, run_delegated_preview};

    fn goreleaser() -> DelegatedTool {
        DelegatedTool {
            tool: "goreleaser".into(),
            args: Some(vec!["release".into(), "--clean".into()]),
            preview: vec!["release".into(), "--snapshot".into(), "--clean".into()],
        }
    }

    #[test]
    fn preview_mode_builds_the_mutation_free_argv_tool_first() {
        let invocation = delegated_request(
            &goreleaser(),
            DelegatedPhaseMode::Preview,
            "/repo",
            vec!["GITHUB_TOKEN".into()],
        );

        assert_eq!(
            invocation.argv,
            vec![
                "goreleaser".to_string(),
                "release".into(),
                "--snapshot".into(),
                "--clean".into(),
            ]
        );
        assert_eq!(invocation.program(), Some("goreleaser"));
        assert_eq!(
            invocation.working_dir(),
            Some(std::path::Path::new("/repo"))
        );
    }

    #[test]
    fn apply_mode_builds_the_mutating_argv_tool_first() {
        let invocation = delegated_request(
            &goreleaser(),
            DelegatedPhaseMode::Apply,
            "/repo",
            Vec::new(),
        );

        assert_eq!(
            invocation.argv,
            vec!["goreleaser".to_string(), "release".into(), "--clean".into()]
        );
    }

    #[test]
    fn secrets_are_named_on_forward_env_never_on_argv() {
        let invocation = delegated_request(
            &goreleaser(),
            DelegatedPhaseMode::Preview,
            "/repo",
            vec!["GITHUB_TOKEN".into(), "REGISTRY_TOKEN".into()],
        );

        assert_eq!(
            invocation.forward_env,
            vec!["GITHUB_TOKEN", "REGISTRY_TOKEN"]
        );
        assert!(
            invocation.argv.iter().all(|arg| !arg.contains("TOKEN")),
            "argv leaked a secret: {:?}",
            invocation.argv
        );
    }

    #[test]
    fn the_engine_drives_a_delegated_preview_argv_first_through_the_runner() {
        let runner = FakeToolRunner::new().with_exit_code(Some(0));

        run_delegated_preview(ReleasePhase::Package, &goreleaser(), &runner, "/repo")
            .expect("preview runs");

        let recorded = runner.requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].program(), Some("goreleaser"));
        assert!(recorded[0].argv.contains(&"--snapshot".to_string()));
    }

    #[test]
    fn a_non_zero_exit_is_a_typed_error_carrying_stderr() {
        let runner = FakeToolRunner::new()
            .with_exit_code(Some(1))
            .with_stderr("goreleaser: build failed");

        let error = run_delegated_preview(ReleasePhase::Package, &goreleaser(), &runner, "/repo")
            .expect_err("non-zero exit fails closed");

        // A delegated tool that ran and failed is an external-service failure
        // (CLI exit 69), not a usage error — pin the taxonomy so the shared
        // `require_success` mapping cannot silently drift.
        assert_eq!(error.code(), rskit_errors::ErrorCode::ExternalService);
        assert!(error.to_string().contains("goreleaser"), "{error}");
        assert!(error.to_string().contains("exited 1"), "{error}");
    }
}
