//! Shared `go` process invocation.
//!
//! Discovery (`go mod edit -json`) and module-set resolution (`go work edit
//! -json`) both shell out to `go` through the same captured, bounded, timed-out
//! path. Every invocation goes through `rskit-process` (never a shell string)
//! and returns typed data + typed errors: no panics, no printing.

use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_process::{CapturedIo, ProcessConfig, ProcessIo, ProcessSpec, run};

/// The go driver name stamped on every discovered workspace and used for every
/// process invocation.
pub(crate) const GO_TOOL: &str = "go";

/// Hard bound on retained `go` JSON output (16 MiB). Large enough for big
/// manifests, bounded so a runaway process cannot exhaust memory.
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Wall-clock bound on a single `go mod edit` / `go work edit` invocation.
const EDIT_TIMEOUT: Duration = Duration::new(120, 0);

/// Run a captured, bounded, timed-out `go` invocation and return its stdout,
/// surfacing timeout / truncation / non-zero exit as typed errors.
///
/// # Errors
/// Returns a typed error when the process times out, exceeds the output bound,
/// or exits non-zero.
pub(crate) fn run_go_json(spec: &ProcessSpec, label: &str) -> AppResult<String> {
    let config = ProcessConfig::default()
        .with_io(ProcessIo::captured(CapturedIo::new()))
        .with_timeout(Some(EDIT_TIMEOUT))
        .with_max_output_bytes(MAX_OUTPUT_BYTES);

    let result = run(spec, &config)?;
    if result.timed_out {
        return Err(AppError::new(
            ErrorCode::Timeout,
            format!("`{label}` timed out"),
        ));
    }
    if result.stdout_truncated {
        return Err(AppError::new(
            ErrorCode::Internal,
            format!("`{label}` output exceeded {MAX_OUTPUT_BYTES} bytes"),
        ));
    }
    if !result.success() {
        return Err(AppError::new(
            ErrorCode::Internal,
            format!(
                "`{label}` failed (exit {:?}): {}",
                result.exit_code,
                result.stderr.trim()
            ),
        ));
    }
    Ok(result.stdout)
}
