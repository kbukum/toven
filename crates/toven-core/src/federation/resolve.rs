//! Four-way ecosystem dispatch and remote-adapter resolution.
//!
//! Extends the load-time three-way config dispatch (loaded / canonical-unloaded
//! / unknown) into the federation four-way:
//!
//! | Case | [`Resolution`] | Behavior |
//! |------|----------------|----------|
//! | linked in this binary | [`Resolution::Linked`] | in-proc configure |
//! | canonical, driver resolved (pin or PATH) | [`Resolution::Driver`] | drive out-of-proc |
//! | canonical, no driver | [`Resolution::Absent`] | warn + skip |
//! | unknown id | [`Resolution::Unknown`] | (already hard-errored at load) |
//!
//! A *resolved* driver that fails to spawn or handshake is a hard PLAN error (a
//! partial federation would corrupt the affected closure); only an *absent*
//! driver is warn + skip.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{ConfiguredAdapter, DriverLocator, Provider};

use super::remote::RemoteAdapter;
use crate::config::{CanonicalRegistry, Document};

/// How an ecosystem id declared in `toven.toml` is served.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum Resolution {
    /// An adapter for this ecosystem is compiled into this binary.
    Linked,
    /// A separately-installed driver binary serves this ecosystem.
    Driver(DriverBinary),
    /// A canonical ecosystem with no installed driver (warn + skip).
    Absent,
    /// An unknown ecosystem id (a typo); hard-errored at config load.
    Unknown,
}

/// A resolved external driver binary.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DriverBinary {
    /// Path or program name to spawn (`<program> __serve`).
    pub program: PathBuf,
    /// Whether the path came from an explicit config pin (vs. PATH convention).
    pub pinned: bool,
}

/// The production locator: scans the process `PATH` for `binary_name`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PathDriverLocator;

impl PathDriverLocator {
    /// Construct a `PATH`-scanning locator.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverLocator for PathDriverLocator {
    fn locate(&self, binary_name: &str) -> AppResult<Option<PathBuf>> {
        let Some(path) = std::env::var_os("PATH") else {
            return Ok(None);
        };
        locate_in_path(std::env::split_paths(&path), binary_name)
    }
}

/// Find an executable `binary_name` in `paths`.
///
/// Requires an executable regular file: a same-named non-executable file on
/// `PATH` must not masquerade as a resolved driver (which would turn an absent
/// driver's warn-and-skip into a hard spawn failure). A filesystem error while
/// inspecting a candidate is propagated rather than collapsed into "absent".
fn locate_in_path(
    paths: impl IntoIterator<Item = PathBuf>,
    binary_name: &str,
) -> AppResult<Option<PathBuf>> {
    for dir in paths {
        let candidate = dir.join(binary_name);
        if rskit_fs::sync_io::file::is_executable(&candidate)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// Classify one ecosystem id into its four-way [`Resolution`].
pub fn resolve_ecosystem(
    id: &EcosystemId,
    loaded: &BTreeSet<EcosystemId>,
    document: &Document,
    canonical: &CanonicalRegistry,
    locator: &dyn DriverLocator,
) -> AppResult<Resolution> {
    if loaded.contains(id) {
        return Ok(Resolution::Linked);
    }
    if !canonical.contains(id) {
        return Ok(Resolution::Unknown);
    }
    if let Some(program) = pinned_driver(document, id)? {
        return Ok(Resolution::Driver(DriverBinary {
            program,
            pinned: true,
        }));
    }
    Ok(locator
        .locate(&driver_binary_name(id))?
        .map_or(Resolution::Absent, |program| {
            Resolution::Driver(DriverBinary {
                program,
                pinned: false,
            })
        }))
}

/// The resolved remote adapters plus the warn-and-skip diagnostics.
#[derive(Default)]
pub struct RemoteResolution {
    /// Configured remote adapters, keyed by ecosystem id.
    pub adapters: BTreeMap<EcosystemId, Box<dyn ConfiguredAdapter>>,
    /// Actionable warnings for canonical ecosystems with no installed driver.
    pub warnings: Vec<String>,
}

/// Resolve and connect every canonical-but-unloaded ecosystem to its driver.
///
/// Linked ecosystems are configured in-proc elsewhere and skipped here. Absent
/// drivers produce a warning. A resolved driver that fails to connect aborts.
///
/// # Errors
/// Propagates a hard PLAN error if a *resolved* driver cannot be spawned or
/// completes its handshake/prefetch with a failure.
pub fn resolve_adapters(
    document: &Document,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
) -> AppResult<RemoteResolution> {
    let loaded: BTreeSet<EcosystemId> = providers
        .iter()
        .map(|provider| provider.ecosystem_id().clone())
        .collect();
    let canonical = CanonicalRegistry::model();
    let mut resolution = RemoteResolution::default();

    for (id, raw) in &document.ecosystems {
        match resolve_ecosystem(id, &loaded, document, &canonical, locator)? {
            Resolution::Linked | Resolution::Unknown => {}
            Resolution::Absent => resolution.warnings.push(absent_hint(id)),
            Resolution::Driver(driver) => {
                let config = driver_config_subtree(raw);
                let adapter = RemoteAdapter::spawn(&driver.program, id.clone(), config)?;
                resolution
                    .adapters
                    .insert(id.clone(), Box::new(adapter) as Box<dyn ConfiguredAdapter>);
            }
        }
    }
    Ok(resolution)
}

/// The conventional driver binary name for an ecosystem id (`toven-<id>`).
pub fn driver_binary_name(id: &EcosystemId) -> String {
    format!("toven-{id}")
}

/// The actionable warn-and-skip hint for an absent driver.
fn absent_hint(id: &EcosystemId) -> String {
    format!(
        "ecosystem '{id}' is declared but no adapter is linked and no driver is installed; skipping (run `toven driver install {id}`)"
    )
}

/// Extract an explicit driver pin: per-section `driver` first, then
/// `[toven.drivers]`.
fn pinned_driver(document: &Document, id: &EcosystemId) -> AppResult<Option<PathBuf>> {
    if let Some(path) = per_section_pin(document, id)? {
        return Ok(Some(path));
    }
    drivers_map_pin(document, id)
}

/// `[ecosystems.<id>].driver = "<path>"`.
fn per_section_pin(document: &Document, id: &EcosystemId) -> AppResult<Option<PathBuf>> {
    let Some(raw) = document.ecosystems.get(id) else {
        return Ok(None);
    };
    let value = toml::Value::try_from(raw).map_err(|error| {
        AppError::invalid_input(
            format!("ecosystems.{id}"),
            format!("could not inspect driver pin: {error}"),
        )
    })?;
    let Some(driver) = value.get("driver") else {
        return Ok(None);
    };
    let Some(path) = driver.as_str() else {
        return Err(AppError::invalid_input(
            format!("ecosystems.{id}.driver"),
            "driver pin must be a string path",
        ));
    };
    Ok(Some(PathBuf::from(path)))
}

/// `[toven.drivers].<id>` as either a bare path string or a `{ path = "..." }`
/// table.
fn drivers_map_pin(document: &Document, id: &EcosystemId) -> AppResult<Option<PathBuf>> {
    let Some(raw) = document.toven.drivers.get(id.as_str()) else {
        return Ok(None);
    };
    let value = toml::Value::try_from(raw).map_err(|error| {
        AppError::invalid_input(
            format!("toven.drivers.{id}"),
            format!("could not inspect driver pin: {error}"),
        )
    })?;
    if let Some(path) = value.as_str() {
        return Ok(Some(PathBuf::from(path)));
    }
    let Some(table) = value.as_table() else {
        return Err(AppError::invalid_input(
            format!("toven.drivers.{id}"),
            "driver pin must be a string path or a table with `path`/`version`",
        ));
    };
    let Some(path) = table.get("path") else {
        return Ok(None);
    };
    let Some(path) = path.as_str() else {
        return Err(AppError::invalid_input(
            format!("toven.drivers.{id}.path"),
            "driver path pin must be a string",
        ));
    };
    Ok(Some(PathBuf::from(path)))
}

/// Umbrella-only keys stripped from an ecosystem subtree before it is handed to
/// a driver: they steer federation in the umbrella and are not part of the
/// adapter's own (`deny_unknown_fields`) configuration schema.
const UMBRELLA_ONLY_KEYS: &[&str] = &["driver"];

/// Strip umbrella-only keys from an `[ecosystems.<id>]` raw subtree, yielding
/// the canonical [`RawValue`] handed to the driver's own `configure`.
fn driver_config_subtree(raw: &rskit_config::RawValue) -> rskit_config::RawValue {
    let mut config = raw.clone();
    if let Some(table) = config.as_object_mut() {
        for key in UMBRELLA_ONLY_KEYS {
            table.remove(*key);
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use toven_model::EcosystemId;
    use toven_testkit::{FakeDriverLocator, FakeProvider};

    use super::{
        PathDriverLocator, Resolution, driver_config_subtree, locate_in_path, resolve_ecosystem,
    };
    use crate::config::{CanonicalRegistry, Document, ProjectConfig, TovenConfig};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).expect("valid id")
    }

    fn raw_subtree(body: &str) -> rskit_config::RawValue {
        let value: toml::Value = toml::from_str(body).expect("valid toml");
        serde_json::to_value(value).expect("json")
    }

    fn document_with(ecosystems: &[(&str, &str)]) -> Document {
        let mut map = std::collections::BTreeMap::new();
        for (id, body) in ecosystems {
            map.insert(eid(id), raw_subtree(body));
        }
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: std::collections::BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: map,
            modules: std::collections::BTreeMap::new(),
            members: Vec::new(),
            hooks: std::collections::BTreeMap::new(),
            units: std::collections::BTreeMap::new(),
        }
    }

    /// A locator that resolves a fixed set of names.
    fn locator(names: &[&str]) -> FakeDriverLocator {
        names
            .iter()
            .fold(FakeDriverLocator::new(), |locator, name| {
                locator.with_conventional(*name)
            })
    }

    #[test]
    fn linked_ecosystem_resolves_in_proc() {
        let rust = FakeProvider::new(eid("rust"));
        let loaded: BTreeSet<EcosystemId> = std::iter::once(eid("rust")).collect();
        let document = document_with(&[("rust", "manifests = []")]);
        let _ = rust;
        let resolution = resolve_ecosystem(
            &eid("rust"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&[]),
        )
        .expect("resolution succeeds");
        assert_eq!(resolution, Resolution::Linked);
    }

    #[test]
    fn canonical_unloaded_with_path_driver_resolves_out_of_proc() {
        let loaded = BTreeSet::new();
        let document = document_with(&[("go", "manifests = []")]);
        let resolution = resolve_ecosystem(
            &eid("go"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&["toven-go"]),
        )
        .expect("resolution succeeds");
        assert!(matches!(resolution, Resolution::Driver(driver) if !driver.pinned));
    }

    #[test]
    fn canonical_unloaded_without_driver_is_absent() {
        let loaded = BTreeSet::new();
        let document = document_with(&[("go", "manifests = []")]);
        let resolution = resolve_ecosystem(
            &eid("go"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&[]),
        )
        .expect("resolution succeeds");
        assert_eq!(resolution, Resolution::Absent);
    }

    #[test]
    fn per_section_pin_takes_precedence_over_path() {
        let loaded = BTreeSet::new();
        let document = document_with(&[("go", "driver = \"/opt/toven-go\"\nmanifests = []")]);
        let resolution = resolve_ecosystem(
            &eid("go"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&["toven-go"]),
        )
        .expect("resolution succeeds");
        assert!(
            matches!(
                &resolution,
                Resolution::Driver(driver)
                    if driver.pinned && driver.program == std::path::Path::new("/opt/toven-go")
            ),
            "expected a pinned /opt/toven-go driver, got {resolution:?}"
        );
    }

    #[test]
    fn unknown_ecosystem_is_unknown() {
        let loaded = BTreeSet::new();
        let document = document_with(&[("go", "manifests = []")]);
        let resolution = resolve_ecosystem(
            &eid("rsut"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&[]),
        )
        .expect("resolution succeeds");
        assert_eq!(resolution, Resolution::Unknown);
        let _ = PathDriverLocator::new();
    }

    #[test]
    fn errored_driver_lookup_propagates_instead_of_reading_as_absent() {
        let loaded = BTreeSet::new();
        let document = document_with(&[("go", "manifests = []")]);
        let locator = FakeDriverLocator::new().with_failing("toven-go");

        let error = resolve_ecosystem(
            &eid("go"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator,
        )
        .expect_err("a failed executability check must not be treated as 'absent'");

        assert_eq!(error.code(), rskit_errors::ErrorCode::Internal);
    }

    #[test]
    fn malformed_per_section_driver_pin_is_invalid_input() {
        let loaded = BTreeSet::new();
        let document = document_with(&[("go", "driver = 123\nmanifests = []")]);

        let error = resolve_ecosystem(
            &eid("go"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&[]),
        )
        .expect_err("non-string per-section driver pin must fail");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(
            error.to_string().contains("ecosystems.go.driver"),
            "error should point at the malformed key: {error}"
        );
    }

    #[test]
    fn malformed_drivers_map_pin_is_invalid_input() {
        let loaded = BTreeSet::new();
        let mut document = document_with(&[("go", "manifests = []")]);
        document.toven.drivers.insert(
            "go".to_string(),
            serde_json::Value::Number(serde_json::Number::from(123)),
        );

        let error = resolve_ecosystem(
            &eid("go"),
            &loaded,
            &document,
            &CanonicalRegistry::model(),
            &locator(&[]),
        )
        .expect_err("non-string/non-table toven driver pin must fail");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(
            error.to_string().contains("toven.drivers.go"),
            "error should point at the malformed key: {error}"
        );
    }

    #[test]
    fn driver_config_subtree_strips_umbrella_only_driver_pin() {
        let raw = raw_subtree("driver = \"/opt/toven-go\"\nmodules = [\"api\"]");
        let config = driver_config_subtree(&raw);
        let table = config.as_object().expect("object");
        assert!(
            !table.contains_key("driver"),
            "driver pin must not reach the adapter"
        );
        assert!(table.contains_key("modules"), "adapter keys must survive");
    }

    #[test]
    fn path_locator_ignores_non_executable_candidates() {
        let root = unique_temp_dir("toven-path-locator");
        fs::create_dir_all(&root).expect("temp dir created");
        let candidate = root.join("toven-go");
        fs::write(&candidate, "#!/bin/sh\n").expect("candidate written");

        let resolved = locate_in_path([root.clone()], "toven-go").expect("locate succeeds");
        assert_eq!(resolved, None);

        fs::remove_file(&candidate).expect("candidate removed");
        fs::remove_dir(&root).expect("temp dir removed");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    #[cfg(unix)]
    #[test]
    fn path_locator_accepts_executable_candidates() {
        use std::os::unix::fs::PermissionsExt as _;

        fn make_executable(path: &Path) {
            let mut permissions = fs::metadata(path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("permissions set");
        }

        let root = unique_temp_dir("toven-path-locator-exec");
        fs::create_dir_all(&root).expect("temp dir created");
        let candidate = root.join("toven-go");
        fs::write(&candidate, "#!/bin/sh\n").expect("candidate written");
        make_executable(&candidate);

        let resolved = locate_in_path([root.clone()], "toven-go").expect("locate succeeds");
        assert_eq!(resolved.as_deref(), Some(candidate.as_path()));

        fs::remove_file(&candidate).expect("candidate removed");
        fs::remove_dir(&root).expect("temp dir removed");
    }
}
