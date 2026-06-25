//! The provisioning verbs: `driver install|list` and `federation sync|status`
//! (cli-taxonomy namespaced surface).
//!
//! Both verb groups are wired into the surface here; their behavior lands in the
//! umbrella-driver (step 11) and cross-repo-federation (step 12) steps. Until
//! then each action returns a clear, typed "not yet implemented" error naming the
//! owning step, so the namespaced dispatch + `--auto-install` gating are
//! exercised end-to-end without pretending to succeed.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult, ErrorCode};

use crate::flags::{DriverAction, FederationAction};

/// Run `toven driver <action>` (currently a typed stub).
///
/// # Errors
/// Always returns an "unimplemented" error until the umbrella-driver step lands.
pub(crate) fn driver(action: &DriverAction, _auto_install: bool) -> AppResult<ExitCode> {
    let action = match action {
        DriverAction::Install { .. } => "install",
        DriverAction::List => "list",
    };
    Err(unimplemented_verb(
        &format!("toven driver {action}"),
        "the out-of-process driver step",
    ))
}

/// Run `toven federation <action>` (currently a typed stub).
///
/// # Errors
/// Always returns an "unimplemented" error until the federation step lands.
pub(crate) fn federation(action: &FederationAction, _auto_install: bool) -> AppResult<ExitCode> {
    let action = match action {
        FederationAction::Sync => "sync",
        FederationAction::Status => "status",
    };
    Err(unimplemented_verb(
        &format!("toven federation {action}"),
        "the cross-repo federation step",
    ))
}

/// Build the typed "verb wired, behavior pending" error.
fn unimplemented_verb(verb: &str, owner: &str) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!("`{verb}` is not yet implemented (lands in {owner})"),
    )
}
