//! [`ToolRunner`] — the synchronous one-shot command-execution seam.
//!
//! The sibling of the async [`CommandRunner`](super::CommandRunner): where that
//! seam drives the streaming, cancellable, persistent-aware APPLY wave walk,
//! this one runs a **single external tool to completion** with captured,
//! bounded output and an optional timeout, and hands back its classified exit.
//! It is the one reusable seam behind every "spawn an argv-first tool, forward
//! named secrets by environment, and gate on its exit" call site — release
//! delegation, artifact verification/signing, hosted-release CLIs, toolchain
//! probes — so none of them re-wire `rskit-process` or re-implement exit
//! mapping. The concrete `rskit-process`-backed runner lives in the engine; a
//! scriptable double lives in `toven-testkit`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};

use super::InvocationEnvironment;

/// One fully-resolved synchronous tool invocation.
///
/// The `argv` is complete and user-owned (`argv[0]` is the program); the runner
/// never rewrites it. By default the child **inherits the parent environment**
/// — external tools spawned by bare name need `PATH`, and most need `HOME` and
/// the ambient VCS/registry configuration; use
/// [`with_environment`](Self::with_environment) to opt into a hermetic policy.
/// Secrets are never placed on `argv` and never logged;
/// [`forward_env`](Self::forward_env) names the ambient secrets the runner
/// resolves and guarantees are present for the child.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolInvocation {
    /// Fully resolved argument vector (`argv[0]` is the program).
    pub argv: Vec<String>,
    /// Working directory the tool runs in, or the process default when unset.
    pub working_dir: Option<PathBuf>,
    /// Explicit environment policy applied to the child (defaults to inheriting
    /// the parent environment).
    pub environment: InvocationEnvironment,
    /// Names of ambient environment variables the child may read (secrets by
    /// name only — the runner resolves each non-empty value at run time).
    pub forward_env: Vec<String>,
    /// Optional wall-clock bound; `None` runs unbounded.
    pub timeout: Option<Duration>,
    /// Optional cap on captured stdout/stderr bytes; `None` uses the runner
    /// default.
    pub max_output_bytes: Option<usize>,
}

impl ToolInvocation {
    /// Construct an invocation from a fully rendered `argv`.
    ///
    /// The child inherits the parent environment by default (see the type-level
    /// docs); override with [`with_environment`](Self::with_environment) for a
    /// hermetic policy.
    #[must_use]
    pub const fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            working_dir: None,
            environment: InvocationEnvironment::inherit_parent(BTreeMap::new()),
            forward_env: Vec::new(),
            timeout: None,
            max_output_bytes: None,
        }
    }

    /// Run the tool in `working_dir`.
    #[must_use]
    pub fn with_working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    /// Set the explicit environment policy.
    #[must_use]
    pub fn with_environment(mut self, environment: InvocationEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// Name the ambient environment variables the child may read.
    #[must_use]
    pub fn with_forward_env(mut self, names: Vec<String>) -> Self {
        self.forward_env = names;
        self
    }

    /// Bound the invocation to `timeout`.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Cap captured stdout/stderr at `max_output_bytes`.
    #[must_use]
    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = Some(max_output_bytes);
        self
    }

    /// The program name (the first argv element), for diagnostics.
    #[must_use]
    pub fn program(&self) -> Option<&str> {
        self.argv.first().map(String::as_str)
    }

    /// The working directory, or `None` for the process default.
    #[must_use]
    pub fn working_dir(&self) -> Option<&Path> {
        self.working_dir.as_deref()
    }
}

/// The classified result of one synchronous tool invocation.
///
/// The runner reports what happened — the exit code and captured output, plus
/// whether the tool was killed by timeout or cancellation — and never decides
/// policy: a non-zero exit is a valid outcome, not an error. Callers gate on it
/// via [`succeeded`](Self::succeeded) or map it to a typed error with the
/// shared [`require_success`](Self::require_success).
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct ToolOutcome {
    /// The tool's exit code, or `None` when it was terminated by a signal.
    pub exit_code: Option<i32>,
    /// Captured standard output (bounded by the runner).
    pub stdout: String,
    /// Captured standard error (bounded by the runner).
    pub stderr: String,
    /// Whether the tool was killed because it exceeded its timeout.
    pub timed_out: bool,
    /// Whether the tool was cancelled by the caller.
    pub cancelled: bool,
}

impl ToolOutcome {
    /// Build a classified outcome from a completed run.
    #[must_use]
    pub fn new(
        exit_code: Option<i32>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Self {
        Self {
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
            cancelled: false,
        }
    }

    /// Mark the outcome as timed out.
    #[must_use]
    pub const fn timed_out_flag(mut self, timed_out: bool) -> Self {
        self.timed_out = timed_out;
        self
    }

    /// Mark the outcome as cancelled.
    #[must_use]
    pub const fn cancelled_flag(mut self, cancelled: bool) -> Self {
        self.cancelled = cancelled;
        self
    }

    /// Whether the tool completed with a zero exit and was neither timed out nor
    /// cancelled.
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(self.exit_code, Some(0)) && !self.timed_out && !self.cancelled
    }

    /// Map a non-success outcome to a typed error, or return `Ok(())`.
    ///
    /// This is the single shared exit-classification used by every fail-closed
    /// tool call site, so a timeout, cancellation, non-zero exit, or
    /// signal-kill produces the same typed error shape everywhere. `context`
    /// names the operation (e.g. `` "delegated package tool `goreleaser`" ``)
    /// and the captured stderr is attached so the failure is actionable.
    ///
    /// # Errors
    /// Returns [`ErrorCode::Cancelled`] / [`ErrorCode::Timeout`] for an
    /// interrupted run and [`ErrorCode::ExternalService`] for a non-zero or
    /// signal-terminated exit.
    pub fn require_success(&self, context: &str) -> AppResult<()> {
        if self.cancelled {
            return Err(
                AppError::new(ErrorCode::Cancelled, format!("{context} was cancelled"))
                    .with_detail("cancelled", true),
            );
        }
        if self.timed_out {
            return Err(
                AppError::new(ErrorCode::Timeout, format!("{context} timed out"))
                    .with_detail("timed_out", true),
            );
        }
        match self.exit_code {
            Some(0) => Ok(()),
            Some(code) => Err(AppError::new(
                ErrorCode::ExternalService,
                format!("{context} exited {code}: {}", self.stderr.trim()),
            )
            .with_detail("exit_code", code)),
            None => Err(AppError::new(
                ErrorCode::ExternalService,
                format!("{context} was terminated by a signal"),
            )
            .with_detail("killed", true)),
        }
    }
}

/// Runs a fully-resolved [`ToolInvocation`] to completion, synchronously.
///
/// The single reusable one-shot execution seam. The caller owns argv
/// construction, working directory, secret selection, and outcome policy; this
/// port owns exactly one thing — spawn the argument vector with captured,
/// bounded output and the requested timeout, forward the named secrets by
/// environment, and report the classified exit. Object-safe so callers can hold
/// it as a `&dyn ToolRunner`; the concrete adapter runs the tool through the
/// rskit process port.
pub trait ToolRunner: Send + Sync {
    /// Run the tool and classify its exit.
    ///
    /// # Errors
    /// Propagates a spawn/IO failure. A non-zero exit, timeout, or cancellation
    /// is *not* an error — it is reported in the returned [`ToolOutcome`] so the
    /// caller can classify it against the operation's guarantees.
    fn run(&self, invocation: &ToolInvocation) -> AppResult<ToolOutcome>;
}

#[cfg(test)]
mod tests {
    use super::{ToolInvocation, ToolOutcome};
    use crate::exec::InvocationEnvPolicy;
    use rskit_errors::ErrorCode;

    #[test]
    fn invocation_is_argv_first_with_optional_policy() {
        let invocation = ToolInvocation::new(vec!["gh".into(), "release".into()])
            .with_working_dir("/repo")
            .with_forward_env(vec!["GITHUB_TOKEN".into()])
            .with_timeout(std::time::Duration::from_secs(5))
            .with_max_output_bytes(1024);

        assert_eq!(invocation.program(), Some("gh"));
        assert_eq!(
            invocation.working_dir(),
            Some(std::path::Path::new("/repo"))
        );
        assert_eq!(invocation.forward_env, vec!["GITHUB_TOKEN".to_string()]);
        assert_eq!(invocation.timeout, Some(std::time::Duration::from_secs(5)));
        assert_eq!(invocation.max_output_bytes, Some(1024));
    }

    #[test]
    fn an_invocation_inherits_the_parent_environment_by_default() {
        // External tools are spawned by bare name and need `PATH`/`HOME`/VCS
        // config, so the default policy must inherit — not clear — the parent
        // environment. Regression guard: a default-empty policy makes every
        // real delegated/one-shot tool spawn with `env_clear()` and fail.
        let invocation = ToolInvocation::new(vec!["goreleaser".into()]);
        assert_eq!(
            invocation.environment.policy,
            InvocationEnvPolicy::InheritParent
        );
    }

    #[test]
    fn succeeded_requires_zero_exit_and_no_interruption() {
        assert!(ToolOutcome::new(Some(0), "", "").succeeded());
        assert!(!ToolOutcome::new(Some(1), "", "").succeeded());
        assert!(
            !ToolOutcome::new(Some(0), "", "")
                .timed_out_flag(true)
                .succeeded()
        );
        assert!(
            !ToolOutcome::new(Some(0), "", "")
                .cancelled_flag(true)
                .succeeded()
        );
    }

    #[test]
    fn require_success_maps_each_failure_mode_to_a_typed_error() {
        assert!(
            ToolOutcome::new(Some(0), "", "")
                .require_success("tool")
                .is_ok()
        );

        let cancelled = ToolOutcome::new(Some(0), "", "")
            .cancelled_flag(true)
            .require_success("tool")
            .expect_err("cancelled is an error");
        assert_eq!(cancelled.code(), ErrorCode::Cancelled);

        let timed_out = ToolOutcome::new(Some(0), "", "")
            .timed_out_flag(true)
            .require_success("tool")
            .expect_err("timeout is an error");
        assert_eq!(timed_out.code(), ErrorCode::Timeout);

        let nonzero = ToolOutcome::new(Some(2), "", "boom")
            .require_success("verify tool `cosign`")
            .expect_err("non-zero is an error");
        assert_eq!(nonzero.code(), ErrorCode::ExternalService);
        assert!(nonzero.to_string().contains("cosign"), "{nonzero}");
        assert!(nonzero.to_string().contains("boom"), "{nonzero}");

        let signal = ToolOutcome::new(None, "", "")
            .require_success("tool")
            .expect_err("signal-kill is an error");
        assert_eq!(signal.code(), ErrorCode::ExternalService);
    }
}
