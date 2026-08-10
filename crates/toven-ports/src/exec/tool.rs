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

/// A secret forwarded to the child under a *different* name than its ambient
/// source.
///
/// The rename companion to [`ToolInvocation::forward_env`]: `source` names the
/// ambient environment variable the runner resolves at run time, and `child`
/// is the variable the tool actually reads (e.g. a configured
/// `MY_REGISTRY_TOKEN` handed to cargo as `CARGO_REGISTRY_TOKEN`). The value is
/// resolved inside the runner and never enters the invocation, so it is never
/// cloned into a recorded [`ToolInvocation`], placed on argv, or logged. Unlike
/// the optional [`forward_env`](ToolInvocation::forward_env), a mapping is
/// *required*: the runner fails closed on an unset or empty `source`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ForwardEnvAs {
    /// Ambient environment variable the runner resolves at run time.
    pub source: String,
    /// Variable name the child process reads the resolved value under.
    pub child: String,
}

impl ForwardEnvAs {
    /// Map an ambient `source` variable onto the `child` name the tool reads.
    #[must_use]
    pub fn new(source: impl Into<String>, child: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            child: child.into(),
        }
    }
}

/// One fully-resolved synchronous tool invocation.
///
/// The `argv` is complete and user-owned (`argv[0]` is the program); the runner
/// never rewrites it. By default the child **inherits the parent environment**
/// — external tools spawned by bare name need `PATH`, and most need `HOME` and
/// the ambient VCS/registry configuration; use
/// [`with_environment`](Self::with_environment) to opt into a hermetic policy.
/// Secrets are never placed on `argv` and never logged, and in neither
/// forwarding form does the value enter the invocation — the runner resolves it
/// at run time. The two forms differ in posture:
/// [`forward_env`](Self::forward_env) names *optional* ambient secrets the child
/// *may* read (an absent one is skipped so the tool can fall back to its own
/// credential resolution), while [`forward_env_as`](Self::forward_env_as) is a
/// *required* rename — the runner resolves it fail-closed, so a configured but
/// absent source is a typed error rather than a silent skip.
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
    /// Ambient secrets forwarded to the child under a *different* name than
    /// their source (secrets by name only — the value never enters the
    /// invocation). Unlike [`forward_env`](Self::forward_env), a mapping here is
    /// *required*: the runner resolves each source fail-closed at run time and
    /// errors on an unset or empty one rather than skipping it.
    pub forward_env_as: Vec<ForwardEnvAs>,
    /// Optional wall-clock bound; `None` runs unbounded.
    pub timeout: Option<Duration>,
    /// Optional cap on captured stdout/stderr bytes; `None` uses the runner
    /// default.
    pub max_output_bytes: Option<usize>,
    /// Optional bytes written to the tool's standard input. `None` gives the
    /// tool no stdin; `Some(bytes)` pipes exactly `bytes` (e.g. release notes
    /// fed to `gh release create --notes-file -`).
    pub stdin: Option<Vec<u8>>,
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
            forward_env_as: Vec::new(),
            timeout: None,
            max_output_bytes: None,
            stdin: None,
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

    /// Forward ambient secrets to the child under a renamed variable.
    #[must_use]
    pub fn with_forward_env_as(mut self, mappings: Vec<ForwardEnvAs>) -> Self {
        self.forward_env_as = mappings;
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

    /// Pipe `stdin` to the tool's standard input.
    #[must_use]
    pub fn with_stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
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

/// Which captured streams overflowed the runner's output bound.
///
/// A grouped companion to a [`ToolOutcome`]: truncation is a property of the
/// captured output, so the per-stream flags travel together rather than as
/// loose booleans on the outcome.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub struct Truncation {
    /// Standard output was truncated at the output bound.
    pub stdout: bool,
    /// Standard error was truncated at the output bound.
    pub stderr: bool,
}

impl Truncation {
    /// Whether either captured stream was truncated at the bound.
    #[must_use]
    pub const fn any(self) -> bool {
        self.stdout || self.stderr
    }
}

/// The classified result of one synchronous tool invocation.
///
/// The runner reports what happened — the exit code and captured output, plus
/// whether the tool was killed by timeout or cancellation and whether its
/// output overflowed the bound — and never decides policy: a non-zero exit is a
/// valid outcome, not an error. Callers gate on it via
/// [`succeeded`](Self::succeeded) or map it to a typed error with the shared
/// [`require_success`](Self::require_success).
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
    /// Which captured streams overflowed the runner's output bound.
    pub truncated: Truncation,
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
            truncated: Truncation {
                stdout: false,
                stderr: false,
            },
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

    /// Record whether captured stdout/stderr were truncated at the bound.
    #[must_use]
    pub const fn truncated_flags(mut self, stdout_truncated: bool, stderr_truncated: bool) -> Self {
        self.truncated = Truncation {
            stdout: stdout_truncated,
            stderr: stderr_truncated,
        };
        self
    }

    /// Whether the tool completed with a zero exit, was neither timed out nor
    /// cancelled, and its captured output was not truncated at the bound.
    ///
    /// Truncation is a failure: a bounded invocation whose output overflowed the
    /// cap yielded incomplete data, so a consumer must never treat it as a clean
    /// success (a `cargo metadata` JSON blob cut mid-stream is not valid).
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(self.exit_code, Some(0))
            && !self.timed_out
            && !self.cancelled
            && !self.truncated.any()
    }

    /// Map a non-success outcome to a typed error, or return `Ok(())`.
    ///
    /// The fail-closed gate for a **delegated external tool** (release
    /// packaging, artifact signing/verification, hosted-release CLIs): a
    /// timeout, cancellation, output-bound overflow, non-zero exit, or
    /// signal-kill becomes the same typed error shape everywhere. A non-zero or
    /// signal exit is classified [`ErrorCode::ExternalService`] — the downstream
    /// tool failed. `context` names the operation (e.g. ``"delegated package
    /// tool `goreleaser`"``) and the captured stderr is attached so the failure
    /// is actionable. For a *local* tool read whose failure is a repository/
    /// configuration problem (`cargo metadata`, `go mod edit`), use
    /// [`require_read_success`](Self::require_read_success) instead.
    ///
    /// # Errors
    /// Returns [`ErrorCode::Cancelled`] / [`ErrorCode::Timeout`] for an
    /// interrupted run, [`ErrorCode::Internal`] for a truncated (bound-
    /// overflowing) capture, and [`ErrorCode::ExternalService`] for a non-zero
    /// or signal-terminated exit.
    pub fn require_success(&self, context: &str) -> AppResult<()> {
        self.classify(context, ErrorCode::ExternalService)
    }

    /// Map a non-success **local tool read** to a typed error, or `Ok(())`.
    ///
    /// The sibling of [`require_success`](Self::require_success) for a tool
    /// Toven runs to read the repository's own state (`cargo metadata`,
    /// `go mod edit -json`): a non-zero or signal exit is a
    /// repository/configuration fault, so it is classified
    /// [`ErrorCode::Internal`] (matching the rskit `ProcessResult::check`
    /// convention) rather than [`ErrorCode::ExternalService`], which would
    /// mislabel a broken manifest as a downstream-service outage.
    ///
    /// # Errors
    /// Returns [`ErrorCode::Cancelled`] / [`ErrorCode::Timeout`] for an
    /// interrupted run and [`ErrorCode::Internal`] for a truncated capture, a
    /// non-zero exit, or a signal-terminated exit.
    pub fn require_read_success(&self, context: &str) -> AppResult<()> {
        self.classify(context, ErrorCode::Internal)
    }

    /// Shared exit classification: interruption and output-bound overflow map to
    /// fixed codes; a non-zero or signal exit maps to `failure_code`.
    fn classify(&self, context: &str, failure_code: ErrorCode) -> AppResult<()> {
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
        if self.truncated.any() {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("{context} output exceeded the captured-output bound"),
            )
            .with_detail("truncated", true));
        }
        match self.exit_code {
            Some(0) => Ok(()),
            Some(code) => Err(AppError::new(
                failure_code,
                format!("{context} exited {code}: {}", self.stderr.trim()),
            )
            .with_detail("exit_code", code)),
            None => Err(AppError::new(
                failure_code,
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
            .with_forward_env_as(vec![super::ForwardEnvAs::new(
                "MY_REGISTRY_TOKEN",
                "CARGO_REGISTRY_TOKEN",
            )])
            .with_timeout(std::time::Duration::from_secs(5))
            .with_max_output_bytes(1024);

        assert_eq!(invocation.program(), Some("gh"));
        assert_eq!(
            invocation.working_dir(),
            Some(std::path::Path::new("/repo"))
        );
        assert_eq!(invocation.forward_env, vec!["GITHUB_TOKEN".to_string()]);
        assert_eq!(
            invocation.forward_env_as,
            vec![super::ForwardEnvAs::new(
                "MY_REGISTRY_TOKEN",
                "CARGO_REGISTRY_TOKEN"
            )]
        );
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
    fn succeeded_requires_zero_exit_no_interruption_and_no_truncation() {
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
        // A zero-exit tool whose captured output overflowed the bound yielded
        // incomplete data, so it must not read as a clean success.
        assert!(
            !ToolOutcome::new(Some(0), "", "")
                .truncated_flags(true, false)
                .succeeded()
        );
    }

    #[test]
    fn require_success_fails_closed_on_a_truncated_capture() {
        // A bounded invocation that overflowed its cap is an error even on a
        // zero exit — the metadata consumer would otherwise parse a JSON blob
        // cut mid-stream. Truncation is a local-bound fault, so it is `Internal`
        // for both the delegated and the read-side classifiers.
        let delegated = ToolOutcome::new(Some(0), "", "")
            .truncated_flags(true, false)
            .require_success("delegated tool")
            .expect_err("truncated output fails closed");
        assert_eq!(delegated.code(), ErrorCode::Internal);
        assert!(delegated.to_string().contains("exceeded"), "{delegated}");

        let read = ToolOutcome::new(Some(0), "", "")
            .truncated_flags(false, true)
            .require_read_success("metadata tool")
            .expect_err("truncated output fails closed");
        assert_eq!(read.code(), ErrorCode::Internal);
    }

    #[test]
    fn require_read_success_classifies_a_local_read_failure_as_internal() {
        // A `cargo metadata` / `go mod edit` failure is a repository/config
        // fault, not a downstream-service outage — so a non-zero or signal exit
        // is `Internal`, matching the rskit `ProcessResult::check` convention.
        let nonzero = ToolOutcome::new(Some(101), "", "no such manifest")
            .require_read_success("metadata tool `cargo`")
            .expect_err("non-zero is an error");
        assert_eq!(nonzero.code(), ErrorCode::Internal);
        assert!(nonzero.to_string().contains("cargo"), "{nonzero}");
        assert!(nonzero.to_string().contains("manifest"), "{nonzero}");

        let signal = ToolOutcome::new(None, "", "")
            .require_read_success("metadata tool `cargo`")
            .expect_err("signal-kill is an error");
        assert_eq!(signal.code(), ErrorCode::Internal);
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
