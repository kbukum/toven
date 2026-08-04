//! [`DelegatedPhase`] — the per-phase delegation contract: back a single release
//! phase with an external tool, invoked argv-first.

use std::path::PathBuf;

use rskit_errors::AppResult;
use toven_model::ReleasePhase;

/// Which mutation posture a delegated phase runs in.
///
/// The engine drives a delegated phase in [`Preview`](Self::Preview) first to
/// honor the flow's mutation-free-preview guarantee, and only in
/// [`Apply`](Self::Apply) once mutation is gated (`--yes` + allowed branch +
/// clean tree). A delegation that cannot preview mutation-free is rejected at
/// config time, never run.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum DelegatedPhaseMode {
    /// A mutation-free preview (the tool's dry-run/plan equivalent).
    Preview,
    /// The real, mutating invocation.
    Apply,
}

impl DelegatedPhaseMode {
    /// Diagnostic label (`preview` or `apply`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Apply => "apply",
        }
    }
}

/// One argv-first invocation of an external tool backing a release phase.
///
/// Fully resolved by the engine before it reaches the runner: `argv` is the
/// complete argument vector (the tool followed by its arguments) and
/// `forward_env` names the environment variables the child may read (a registry
/// token, a forge token). Secrets travel **only** through the child-process
/// environment referenced by name here — never in `argv`, never in logs.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct DelegatedPhaseRequest {
    /// The release phase being delegated.
    pub phase: ReleasePhase,
    /// The complete argument vector: the executable followed by its arguments.
    pub argv: Vec<String>,
    /// The posture the invocation runs in.
    pub mode: DelegatedPhaseMode,
    /// The working directory the tool runs in (the repository root).
    pub working_dir: PathBuf,
    /// Names of environment variables the child process may read (secrets by
    /// name only — never their values).
    pub forward_env: Vec<String>,
}

impl DelegatedPhaseRequest {
    /// Build a delegated-phase request from its fully-resolved parts.
    #[must_use]
    pub fn new(
        phase: ReleasePhase,
        argv: Vec<String>,
        mode: DelegatedPhaseMode,
        working_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            phase,
            argv,
            mode,
            working_dir: working_dir.into(),
            forward_env: Vec::new(),
        }
    }

    /// Name the environment variables the child process may read.
    #[must_use]
    pub fn with_forward_env(mut self, names: Vec<String>) -> Self {
        self.forward_env = names;
        self
    }

    /// The tool name (the first argv element), for diagnostics.
    #[must_use]
    pub fn tool(&self) -> Option<&str> {
        self.argv.first().map(String::as_str)
    }
}

/// The classified result of one delegated-phase invocation.
///
/// The engine maps this generic exit classification into the phase's own typed
/// outcome (an artifact, a publish classification) at the phase call site; the
/// runner only reports whether the tool succeeded and what it emitted.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct DelegatedPhaseOutcome {
    /// The tool's exit code, or `None` when it was terminated by a signal.
    pub exit_code: Option<i32>,
    /// Captured standard output (bounded by the runner).
    pub stdout: String,
    /// Captured standard error (bounded by the runner).
    pub stderr: String,
}

impl DelegatedPhaseOutcome {
    /// Build a classified outcome from a tool's exit code and captured output.
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
        }
    }

    /// Whether the tool exited zero.
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(self.exit_code, Some(0))
    }
}

/// Run an external tool that backs a single release phase, argv-first.
///
/// The delegation seam. The engine owns selection, ordering, readiness, safety,
/// and reporting; this port owns exactly one thing — spawning the external tool
/// as an argument vector, forwarding the named secrets through the child
/// environment, and reporting its classified exit. Object-safe so the engine can
/// hold it as a trait object; the concrete adapter runs the tool through the
/// rskit process port.
pub trait DelegatedPhase {
    /// Run the delegated tool for one phase and classify its exit.
    ///
    /// # Errors
    /// Propagates a spawn/IO failure; a non-zero tool exit is reported in the
    /// returned [`DelegatedPhaseOutcome`], not as an error, so the engine can
    /// classify it against the phase's guarantees.
    fn run(&self, request: &DelegatedPhaseRequest) -> AppResult<DelegatedPhaseOutcome>;
}
