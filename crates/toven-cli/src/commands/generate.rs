//! The `generate` verb (cli-taxonomy flat surface).
//!
//! The verb is wired into the surface here; its behavior — scaffolding and
//! regenerating `[ecosystems.<id>]` sections from each provider's `scaffold` —
//! lands in step 13. Until then it returns a clear, typed "not yet implemented"
//! error so the dispatch path is exercised without pretending to succeed.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult, ErrorCode};

/// Run `toven generate` (currently a typed stub).
///
/// # Errors
/// Always returns an "unimplemented" error until step 13 lands the behavior.
pub(crate) fn execute() -> AppResult<ExitCode> {
    Err(AppError::new(
        ErrorCode::Internal,
        "`toven generate` is not yet implemented (lands in the scaffolding step)",
    ))
}
