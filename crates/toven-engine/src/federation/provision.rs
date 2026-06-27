//! Explicit driver provisioning — `toven driver install|list` and
//! `toven federation sync|status`.
//!
//! A normal run **never installs**: an absent driver is warn + skip
//! ([`resolve`](super::resolve)). Provisioning is this separate, opt-in surface
//! so runs stay pure and reproducible and the network + supply-chain surface is
//! isolated to an explicit action. Drivers are installed argv-only via
//! `cargo install toven-<id>` through rskit-process; CI can pin an exact version
//! in `[toven.drivers]`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_process::{InheritedIo, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_model::EcosystemId;

use super::resolve::{DriverLocator, Resolution, resolve_ecosystem};
use crate::config::{CanonicalRegistry, Document};

/// The resolved provisioning state of one canonical ecosystem.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DriverStatus {
    /// The ecosystem id.
    pub id: EcosystemId,
    /// How it is (or is not) currently served.
    pub state: DriverState,
}

/// How a canonical ecosystem is served, for the status views.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum DriverState {
    /// An adapter is compiled into this binary.
    Linked,
    /// An explicitly pinned driver binary serves it.
    Pinned(PathBuf),
    /// An explicitly pinned driver path is missing or not executable.
    PinnedUnavailable(PathBuf),
    /// A driver was found on `PATH`.
    OnPath(PathBuf),
    /// No adapter and no driver — declared sections would warn + skip.
    Absent,
}

/// Install (or update) the driver for `id` via `cargo install toven-<id>`.
///
/// When `version` is set, `--version <version>` pins the exact release for
/// reproducible CI. Output streams straight to the inherited terminal — the
/// engine does not capture or reprint it.
///
/// # Errors
/// Returns an error if `cargo` cannot be launched or exits non-zero.
pub fn install_driver(id: &EcosystemId, version: Option<&str>) -> AppResult<()> {
    let crate_name = format!("toven-{id}");
    let mut spec = ProcessSpec::new("cargo").arg("install").arg(&crate_name);
    if let Some(version) = version {
        spec = spec.arg("--version").arg(version);
    }
    // Installs are long and interactive-ish; inherit stdio and lift the timeout.
    let config = ProcessConfig::default()
        .with_timeout(None)
        .with_io(ProcessIo::inherited(InheritedIo::new()));

    let result = run(&spec, &config)?;
    if result.exit_code == Some(0) {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::ServiceUnavailable,
            format!(
                "`cargo install {crate_name}` exited with status {}",
                result
                    .exit_code
                    .map_or_else(|| "signal".to_string(), |code| code.to_string())
            ),
        ))
    }
}

/// Read an exact version pin for `id` from `[toven.drivers].<id> = { version = "…" }`.
#[must_use]
pub fn version_pin(document: &Document, id: &EcosystemId) -> Option<String> {
    let raw = document.toven.drivers.get(id.as_str())?;
    let value = toml::Value::try_from(raw).ok()?;
    Some(value.get("version")?.as_str()?.to_string())
}

/// The set of ecosystems this project references: every declared `[ecosystems.*]`
/// section plus every `[toven.drivers]` pin.
///
/// Auto-install (`--auto-install`) is scoped to this set so it never provisions
/// drivers for canonical ecosystems the project does not actually use, keeping
/// network and toolchain work to what the config asks for. Invalid driver-pin ids
/// are ignored here (the strict [`federation_sync`] path reports those).
#[must_use]
pub fn referenced_ecosystems(document: &Document) -> BTreeSet<EcosystemId> {
    let mut ids: BTreeSet<EcosystemId> = document.ecosystems.keys().cloned().collect();
    for id in document.toven.drivers.keys() {
        if let Ok(ecosystem) = EcosystemId::new(id) {
            ids.insert(ecosystem);
        }
    }
    ids
}

/// Build the provisioning status of every canonical ecosystem.
#[must_use]
pub fn list_drivers(
    document: &Document,
    loaded: &BTreeSet<EcosystemId>,
    locator: &dyn DriverLocator,
) -> Vec<DriverStatus> {
    let canonical = CanonicalRegistry::model();
    let mut statuses = Vec::new();
    for id in canonical.ids() {
        let state = match resolve_ecosystem(&id, loaded, document, &canonical, locator) {
            Resolution::Linked => DriverState::Linked,
            Resolution::Driver(driver)
                if driver.pinned && program_is_executable(&driver.program) =>
            {
                DriverState::Pinned(driver.program)
            }
            Resolution::Driver(driver) if driver.pinned => {
                DriverState::PinnedUnavailable(driver.program)
            }
            Resolution::Driver(driver) => DriverState::OnPath(driver.program),
            Resolution::Absent | Resolution::Unknown => DriverState::Absent,
        };
        statuses.push(DriverStatus { id, state });
    }
    statuses
}

/// Provision every driver pinned in `[toven.drivers]`, returning the installed ids.
///
/// Deterministic CI provisioning: one `toven federation sync` installs each
/// pinned driver at its pinned version.
///
/// # Errors
/// Propagates the first install failure (the run aborts rather than leave a
/// partially provisioned federation).
pub fn federation_sync(document: &Document) -> AppResult<Vec<EcosystemId>> {
    let mut installed = Vec::new();
    for id in document.toven.drivers.keys() {
        let ecosystem = EcosystemId::new(id).map_err(|error| {
            AppError::invalid_input(
                "toven.drivers",
                format!("invalid driver id '{id}': {error}"),
            )
        })?;
        if let Some(path) = path_pin(document, &ecosystem) {
            return Err(AppError::invalid_input(
                format!("toven.drivers.{id}"),
                format!(
                    "path-pinned driver '{id}' cannot be installed by `toven federation sync`; validate the configured binary separately at {}",
                    path.display()
                ),
            ));
        }
        let version = version_pin(document, &ecosystem);
        install_driver(&ecosystem, version.as_deref())?;
        installed.push(ecosystem);
    }
    Ok(installed)
}

/// Whether `program` exists and is executable (used by provisioning status views).
#[must_use]
pub fn program_is_executable(program: &Path) -> bool {
    rskit_fs::sync_io::file::is_executable(program).unwrap_or(false)
}

/// Read a path pin from `[toven.drivers].<id>` as a bare string or `{ path = "..." }`.
fn path_pin(document: &Document, id: &EcosystemId) -> Option<PathBuf> {
    let raw = document.toven.drivers.get(id.as_str())?;
    let value = toml::Value::try_from(raw).ok()?;
    if let Some(path) = value.as_str() {
        return Some(PathBuf::from(path));
    }
    Some(PathBuf::from(value.get("path")?.as_str()?))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use rskit_errors::ErrorCode;
    use toven_model::EcosystemId;

    use super::{DriverState, federation_sync, list_drivers, referenced_ecosystems, version_pin};
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::federation::resolve::{DriverLocator, PathDriverLocator};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).expect("valid id")
    }

    fn raw_subtree(body: &str) -> rskit_config::RawValue {
        let value: toml::Value = toml::from_str(body).expect("valid toml");
        serde_json::to_value(value).expect("json")
    }

    fn document(drivers: &[(&str, &str)]) -> Document {
        let mut map = BTreeMap::new();
        for (id, body) in drivers {
            map.insert((*id).to_string(), raw_subtree(body));
        }
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig {
                drivers: map,
                ..TovenConfig::default()
            },
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: BTreeMap::new(),
            members: Vec::new(),
        }
    }

    struct NoLocator;
    impl DriverLocator for NoLocator {
        fn locate(&self, _binary_name: &str) -> Option<std::path::PathBuf> {
            None
        }
    }

    #[test]
    fn version_pin_reads_table_version() {
        let document = document(&[("go", "version = \"1.2.3\"")]);
        assert_eq!(version_pin(&document, &eid("go")).as_deref(), Some("1.2.3"));
        assert_eq!(version_pin(&document, &eid("rust")), None);
    }

    #[test]
    fn list_marks_loaded_as_linked_and_others_absent() {
        let document = document(&[]);
        let loaded: BTreeSet<EcosystemId> = std::iter::once(eid("rust")).collect();
        let statuses = list_drivers(&document, &loaded, &NoLocator);
        let rust = statuses
            .iter()
            .find(|s| s.id == eid("rust"))
            .expect("rust listed");
        assert_eq!(rust.state, DriverState::Linked);
        assert!(statuses.iter().any(|s| s.state == DriverState::Absent));
        let _ = PathDriverLocator::new();
    }

    #[test]
    fn list_marks_broken_path_pins_as_unavailable() {
        let mut document = document(&[]);
        document.toven.drivers.insert(
            "go".to_string(),
            serde_json::Value::String("/definitely/missing/toven-go".to_string()),
        );

        let statuses = list_drivers(&document, &BTreeSet::new(), &NoLocator);
        let go = statuses
            .iter()
            .find(|s| s.id == eid("go"))
            .expect("go listed");

        assert_eq!(
            go.state,
            DriverState::PinnedUnavailable(PathBuf::from("/definitely/missing/toven-go"))
        );
    }

    #[test]
    fn federation_sync_rejects_path_pins_without_installing() {
        let document = document(&[("go", "path = \"/opt/toven-go\"")]);

        let error = federation_sync(&document).expect_err("path pins are not installable");

        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert!(
            error.to_string().contains("path-pinned driver"),
            "error should explain path pins are not installable: {error}"
        );
    }

    #[test]
    fn referenced_ecosystems_unions_declared_sections_and_driver_pins() {
        // A driver pin (`go`) and a declared `[ecosystems.rust]` section are both
        // referenced; a canonical ecosystem with neither is not.
        let mut document = document(&[("go", "version = \"1.2.3\"")]);
        document
            .ecosystems
            .insert(eid("rust"), raw_subtree("manifests = []"));

        let referenced = referenced_ecosystems(&document);
        assert!(referenced.contains(&eid("go")), "driver pin is referenced");
        assert!(
            referenced.contains(&eid("rust")),
            "declared section is referenced"
        );
        assert!(
            !referenced.contains(&eid("python")),
            "an unreferenced canonical ecosystem is excluded"
        );
    }
}
