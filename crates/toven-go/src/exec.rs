//! Shared `go` process invocation.
//!
//! Discovery (`go mod edit -json`), module-set resolution (`go work edit
//! -json`), and release manifest edits all run `go` through one [`GoTool`]: the
//! injected [`ToolRunner`] seam (never a shell string). Every call is captured,
//! bounded, and timed out, and returns typed data + typed errors: no panics, no
//! printing.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{InvocationEnvironment, ToolInvocation, ToolRunner};

/// The go driver name stamped on every discovered workspace and used for every
/// process invocation.
pub(crate) const GO_TOOL: &str = "go";

/// Hard bound on retained `go` JSON output (16 MiB). Large enough for big
/// manifests, bounded so a runaway process cannot exhaust memory.
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Wall-clock bound on a single `go mod edit` / `go work edit` invocation.
const EDIT_TIMEOUT: Duration = Duration::new(120, 0);

/// The runner every Toven-owned `go` invocation uses.
#[derive(Clone)]
pub(crate) struct GoTool {
    runner: Arc<dyn ToolRunner>,
}

impl GoTool {
    /// Wrap the injected runner for Toven's own `go` reads and edits.
    #[must_use]
    pub(crate) const fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self { runner }
    }

    /// A `go` invocation that inherits the user's environment unchanged.
    ///
    /// Toven does not set `GOTOOLCHAIN`: the user's value, or Go's own default
    /// (`auto`, which follows the `go`/`toolchain` lines of the enclosing
    /// `go.mod`/`go.work`), applies exactly as it does for the user's tasks, so
    /// discovery reads manifests with the same Go that builds them.
    #[must_use]
    pub(crate) fn command<I, S>(args: I, working_dir: &Path) -> ToolInvocation
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let full_argv = std::iter::once(GO_TOOL.to_string())
            .chain(args.into_iter().map(Into::into))
            .collect();
        ToolInvocation::new(full_argv)
            .with_working_dir(working_dir)
            .with_environment(InvocationEnvironment::inherit_parent(BTreeMap::new()))
    }

    /// Run a captured, bounded, timed-out `go` invocation and return its
    /// stdout, surfacing timeout / non-zero exit as typed errors.
    ///
    /// # Errors
    /// Returns a typed error when the process times out, overflows its output
    /// bound, or exits non-zero. A non-zero exit carries a toolchain hint,
    /// because the usual cause is a Go too old to read the repository's
    /// manifests.
    pub(crate) fn run_json(&self, invocation: ToolInvocation, label: &str) -> AppResult<String> {
        let invocation = invocation
            .with_timeout(EDIT_TIMEOUT)
            .with_max_output_bytes(MAX_OUTPUT_BYTES);

        let outcome = self.runner.run(&invocation)?;
        if outcome.timed_out {
            return Err(AppError::new(
                ErrorCode::Timeout,
                format!("`{label}` timed out"),
            ));
        }
        // `go mod edit -json` reads the repository's own module graph, so a failure
        // is a repository/config fault (`Internal`), not a downstream-service outage.
        outcome
            .require_read_success(&format!("go tool `go` ({label})"))
            .map_err(|error| {
                let exited_non_zero = outcome.exit_code.is_some_and(|code| code != 0);
                with_toolchain_hint(error, exited_non_zero)
            })?;
        Ok(outcome.stdout)
    }
}

/// Add the toolchain hint to a `go` that exited non-zero.
fn with_toolchain_hint(error: AppError, exited_non_zero: bool) -> AppError {
    if exited_non_zero {
        error.hint(
            "Hint: Toven runs `go` with your environment, so it uses the `go` on PATH and your \
             GOTOOLCHAIN setting. If that Go is older than the repository needs, put a newer `go` \
             first on PATH or set GOTOOLCHAIN (for example GOTOOLCHAIN=go1.27.1, or auto).",
        )
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rskit_errors::ErrorCode;
    use toven_ports::InvocationEnvPolicy;
    use toven_testkit::doubles::FakeToolRunner;

    use super::{EDIT_TIMEOUT, GoTool, MAX_OUTPUT_BYTES};

    fn tool(runner: FakeToolRunner) -> (GoTool, Arc<FakeToolRunner>) {
        let runner = Arc::new(runner);
        (GoTool::new(runner.clone()), runner)
    }

    #[test]
    fn go_invocation_inherits_the_user_environment_unchanged() {
        let invocation = GoTool::command(["mod", "edit", "-json", "go.mod"], Path::new("/repo"));

        assert_eq!(
            invocation.argv,
            ["go", "mod", "edit", "-json", "go.mod"].map(String::from)
        );
        assert_eq!(invocation.working_dir(), Some(Path::new("/repo")));
        assert_eq!(
            invocation.environment.policy,
            InvocationEnvPolicy::InheritParent
        );
        // No override: the user's GOTOOLCHAIN (or Go's own default) applies.
        assert!(invocation.environment.vars.is_empty());
    }

    #[test]
    fn run_json_uses_the_injected_tool_runner() {
        let (go, runner) = tool(
            FakeToolRunner::new()
                .with_exit_code(Some(2))
                .with_stderr("edit failed"),
        );

        let error = go
            .run_json(
                GoTool::command(["work", "edit"], Path::new("/repo")),
                "go work edit",
            )
            .expect_err("non-zero go is rejected");

        // A `go mod edit` failure is a repository/config fault, so it classifies
        // `Internal` — not the delegated-tool `ExternalService`.
        assert_eq!(error.code(), ErrorCode::Internal);
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].argv, ["go", "work", "edit"].map(String::from));
        assert_eq!(requests[0].timeout, Some(EDIT_TIMEOUT));
        assert_eq!(requests[0].max_output_bytes, Some(MAX_OUTPUT_BYTES));
    }

    #[test]
    fn non_zero_exit_hints_at_the_toolchain() {
        let (go, _) = tool(
            FakeToolRunner::new()
                .with_exit_code(Some(1))
                .with_stderr("go: errors parsing go.mod: unknown block type: tool"),
        );
        let error = go
            .run_json(
                GoTool::command(["mod", "edit"], Path::new("/repo")),
                "go mod edit",
            )
            .expect_err("rejected");
        assert!(error.to_string().contains("GOTOOLCHAIN"), "{error}");
    }

    #[test]
    fn run_json_output_that_overflows_the_bound_fails_closed() {
        // A truncated `go mod edit -json` capture is incomplete JSON; the seam
        // must reject it rather than hand a cut document to the caller.
        let (go, _) = tool(
            FakeToolRunner::new()
                .with_exit_code(Some(0))
                .with_stdout("{ \"Module\": {")
                .with_truncated(true, false),
        );

        let error = go
            .run_json(
                GoTool::command(["mod", "edit", "-json", "go.mod"], Path::new("/repo")),
                "go mod edit",
            )
            .expect_err("a truncated capture is rejected");

        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(error.to_string().contains("exceeded"), "{error}");
        assert!(!error.to_string().contains("GOTOOLCHAIN"), "{error}");
    }
}
