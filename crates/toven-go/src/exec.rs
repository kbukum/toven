//! Shared `go` process invocation.
//!
//! Discovery (`go mod edit -json`) and module-set resolution (`go work edit
//! -json`) both shell out to `go` through the same captured, bounded, timed-out
//! path. Every invocation goes through the injected [`ToolRunner`] seam (never a
//! shell string) and returns typed data + typed errors: no panics, no printing.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{InvocationEnvironment, ToolInvocation, ToolRunner};

/// The go driver name stamped on every discovered workspace and used for every
/// process invocation.
pub(crate) const GO_TOOL: &str = "go";

/// A `go` invocation pinned to the locally installed toolchain.
///
/// Discovery only *reads* manifests (`go mod edit -json` / `go work edit
/// -json`), yet Go's default `GOTOOLCHAIN=auto` still consults the `go`
/// directive in `go.mod`/`go.work` and downloads a newer toolchain when the
/// declared version exceeds the installed one. Pinning `GOTOOLCHAIN=local`
/// keeps discovery hermetic and offline: reading a manifest never triggers a
/// network toolchain download (the local `go` parses any newer directive), so
/// module resolution stays deterministic regardless of the repo's declared Go
/// version. Every other environment variable (`PATH`, `HOME`, …) is inherited.
pub(crate) fn go_command<I, S>(args: I, working_dir: &Path) -> ToolInvocation
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut env = BTreeMap::new();
    env.insert("GOTOOLCHAIN".to_string(), "local".to_string());
    let full_argv = std::iter::once(GO_TOOL.to_string())
        .chain(args.into_iter().map(Into::into))
        .collect();
    ToolInvocation::new(full_argv)
        .with_working_dir(working_dir)
        .with_environment(InvocationEnvironment::inherit_parent(env))
}

/// Hard bound on retained `go` JSON output (16 MiB). Large enough for big
/// manifests, bounded so a runaway process cannot exhaust memory.
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Wall-clock bound on a single `go mod edit` / `go work edit` invocation.
const EDIT_TIMEOUT: Duration = Duration::new(120, 0);

/// Run a captured, bounded, timed-out `go` invocation and return its stdout,
/// surfacing timeout / non-zero exit as typed errors.
///
/// # Errors
/// Returns a typed error when the process times out or exits non-zero.
pub(crate) fn run_go_json(
    invocation: ToolInvocation,
    label: &str,
    runner: &dyn ToolRunner,
) -> AppResult<String> {
    let invocation = invocation
        .with_timeout(EDIT_TIMEOUT)
        .with_max_output_bytes(MAX_OUTPUT_BYTES);

    let outcome = runner.run(&invocation)?;
    if outcome.timed_out {
        return Err(AppError::new(
            ErrorCode::Timeout,
            format!("`{label}` timed out"),
        ));
    }
    if !outcome.succeeded() {
        outcome.require_success(&format!("go tool `go` ({label})"))?;
    }
    Ok(outcome.stdout)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rskit_errors::ErrorCode;
    use toven_ports::InvocationEnvPolicy;
    use toven_testkit::doubles::FakeToolRunner;

    use super::{go_command, run_go_json};

    #[test]
    fn go_invocation_preserves_parent_environment_with_local_toolchain_override() {
        let invocation = go_command(["mod", "edit", "-json", "go.mod"], Path::new("/repo"));

        assert_eq!(
            invocation.argv,
            vec![
                "go".to_string(),
                "mod".to_string(),
                "edit".to_string(),
                "-json".to_string(),
                "go.mod".to_string(),
            ]
        );
        assert_eq!(invocation.working_dir(), Some(Path::new("/repo")));
        assert_eq!(
            invocation.environment.policy,
            InvocationEnvPolicy::InheritParent
        );
        assert_eq!(
            invocation.environment.vars.get("GOTOOLCHAIN"),
            Some(&"local".to_string())
        );
    }

    #[test]
    fn run_go_json_uses_the_injected_tool_runner() {
        let runner = FakeToolRunner::new()
            .with_exit_code(Some(2))
            .with_stderr("edit failed");

        let error = run_go_json(
            go_command(["work", "edit"], Path::new("/repo")),
            "go work edit",
            &runner,
        )
        .expect_err("non-zero go is rejected");

        assert_eq!(error.code(), ErrorCode::ExternalService);
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].argv,
            vec!["go".to_string(), "work".to_string(), "edit".to_string()]
        );
        assert_eq!(requests[0].timeout, Some(super::EDIT_TIMEOUT));
        assert_eq!(requests[0].max_output_bytes, Some(super::MAX_OUTPUT_BYTES));
    }
}
