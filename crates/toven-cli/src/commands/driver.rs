//! The provisioning verbs (`driver install|list`, `federation sync|status`) and
//! the hidden `__serve` port-server entry (cli-taxonomy namespaced surface).
//!
//! These are the explicit, opt-in provisioning surface: a normal run never
//! installs a driver (an absent driver is warn + skip in the engine's four-way
//! dispatch), so installing/syncing is isolated here behind its own verbs. Each
//! action is a thin caller over the engine's
//! [`federation::provision`](toven_engine::federation::provision) functions; all
//! human-facing lines go to **stderr** so `stdout` stays reserved for the JSONL
//! machine stream and the `__serve` frame transport.

use rskit_cli::{ErrorRenderer, ExitCode};
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_engine::federation::provision::{
    self, DriverState, DriverStatus, install_driver, version_pin,
};
use toven_engine::federation::resolve::PathDriverLocator;
use toven_model::EcosystemId;
use toven_ports::Provider;

use crate::flags::{DriverAction, FederationAction};
use crate::host::Project;

/// Run the hidden `toven-<eco> __serve` port-server loop over stdio.
///
/// Drives the engine's framed [`serve`](toven_engine::federation::serve) loop
/// with the in-proc `providers`: stdin/stdout carry the request/response frame
/// stream and any failure is rendered to stderr. Never panics — a transport or
/// handshake failure maps to a process [`ExitCode`].
#[must_use]
pub(crate) fn serve(providers: &[&dyn Provider]) -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match toven_engine::federation::serve(providers, stdin.lock(), stdout.lock()) {
        Ok(()) => ExitCode::Success,
        Err(error) => {
            let (rendered, code) = ErrorRenderer::default().render(&error);
            eprintln!("{rendered}");
            code
        }
    }
}

/// Run the hidden `toven-<eco> __init` config-less wizard exchange.
///
/// Drives the engine's framed
/// [`serve_wizard`](toven_engine::federation::serve_wizard) loop with the
/// in-proc `providers`: stdin carries the umbrella's wizard probe/answers, stdout
/// the reply frames, and any failure is rendered to stderr. Never panics.
#[must_use]
pub(crate) fn init_wizard(providers: &[&dyn Provider]) -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match toven_engine::federation::serve_wizard(providers, stdin.lock(), stdout.lock()) {
        Ok(()) => ExitCode::Success,
        Err(error) => {
            let (rendered, code) = ErrorRenderer::default().render(&error);
            eprintln!("{rendered}");
            code
        }
    }
}

/// Run `toven driver <action>`.
///
/// # Errors
/// Propagates an install failure (`cargo install` could not run or exited
/// non-zero) or an invalid ecosystem id.
pub(crate) fn driver(
    providers: &[&dyn Provider],
    project: &Project,
    action: &DriverAction,
    auto_install: bool,
) -> AppResult<ExitCode> {
    match action {
        DriverAction::Install { id } => install(project, id),
        DriverAction::List => {
            if auto_install {
                install_absent(providers, project)?;
            }
            report_statuses("driver", providers, project)?;
            Ok(ExitCode::Success)
        }
    }
}

/// Run `toven federation <action>`.
///
/// # Errors
/// Propagates the first install failure from `federation sync` or an invalid
/// pinned driver id.
pub(crate) fn federation(
    providers: &[&dyn Provider],
    project: &Project,
    action: &FederationAction,
    auto_install: bool,
) -> AppResult<ExitCode> {
    match action {
        FederationAction::Sync => {
            let installed = provision::federation_sync(&project.document)?;
            if auto_install {
                install_absent(providers, project)?;
            }
            if installed.is_empty() {
                eprintln!(
                    "federation sync: no pinned drivers in [toven.drivers]; nothing to install"
                );
            } else {
                for id in &installed {
                    eprintln!("federation sync: installed driver for '{id}'");
                }
            }
            Ok(ExitCode::Success)
        }
        FederationAction::Status => {
            report_statuses("federation", providers, project)?;
            Ok(ExitCode::Success)
        }
    }
}

/// Install (or update) the driver for `id`, pinning the `[toven.drivers]` version
/// if one is configured.
fn install(project: &Project, id: &str) -> AppResult<ExitCode> {
    let ecosystem = parse_id(id)?;
    let version = version_pin(&project.document, &ecosystem);
    install_driver(&ecosystem, version.as_deref())?;
    eprintln!("installed driver for '{ecosystem}'");
    Ok(ExitCode::Success)
}

/// Install the driver for every **referenced** canonical ecosystem currently
/// resolved as [`DriverState::Absent`].
///
/// Scoped to ecosystems the project actually references (declared `[ecosystems.*]`
/// sections and `[toven.drivers]` pins) so `--auto-install` never provisions
/// drivers for canonical ecosystems this project does not use.
fn install_absent(providers: &[&dyn Provider], project: &Project) -> AppResult<()> {
    let referenced = provision::referenced_ecosystems(&project.document);
    for status in statuses(providers, project)? {
        if status.state == DriverState::Absent && referenced.contains(&status.id) {
            let version = version_pin(&project.document, &status.id);
            install_driver(&status.id, version.as_deref())?;
            eprintln!("auto-installed driver for '{}'", status.id);
        }
    }
    Ok(())
}

/// Render every canonical ecosystem's provisioning state to stderr.
fn report_statuses(verb: &str, providers: &[&dyn Provider], project: &Project) -> AppResult<()> {
    for status in statuses(providers, project)? {
        eprintln!("{verb}: {} -> {}", status.id, describe(&status.state));
    }
    Ok(())
}

/// The provisioning status of every canonical ecosystem for this project.
fn statuses(providers: &[&dyn Provider], project: &Project) -> AppResult<Vec<DriverStatus>> {
    let loaded = providers
        .iter()
        .map(|provider| provider.ecosystem_id().clone())
        .collect();
    provision::list_drivers(&project.document, &loaded, &PathDriverLocator::new())
}

/// A short human label for a driver state.
fn describe(state: &DriverState) -> String {
    match state {
        DriverState::Linked => "linked (in this binary)".to_string(),
        DriverState::Pinned(path) => format!("pinned driver {}", path.display()),
        DriverState::PinnedUnavailable(path) => {
            format!(
                "pinned driver unavailable (missing or not executable) {}",
                path.display()
            )
        }
        DriverState::OnPath(path) => format!("driver on PATH {}", path.display()),
        DriverState::Absent => "absent (run `toven driver install <id>`)".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Parse a user-supplied ecosystem id, surfacing a typed usage error.
fn parse_id(id: &str) -> AppResult<EcosystemId> {
    EcosystemId::new(id).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidInput,
            format!("invalid ecosystem id '{id}': {error}"),
        )
    })
}
