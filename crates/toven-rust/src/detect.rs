//! Config-less detection: find the Cargo workspace(s) under a root and probe
//! their test tooling.
//!
//! A root `Cargo.toml` is used verbatim. Otherwise Toven discovers first-level
//! nested Cargo manifests (for repositories that group several workspaces under
//! subdirectories, e.g. `core/`, `contrib/`, `examples/`), skipping any path
//! ignored by Git so vendored or build-output trees never leak into onboarding.

use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;
use toven_ports::Detection;

use crate::manifests::{discover_manifests, existing_lockfiles};

/// The nextest config that marks a workspace configured for `cargo-nextest`.
const NEXTEST_CONFIG: &str = ".config/nextest.toml";

/// The adapter-owned facts a Rust [`Detection`] carries to
/// [`render`](crate::render): the discovered manifests, their existing
/// lockfiles, and whether any workspace is configured for `cargo-nextest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RustFacts {
    /// The repo-relative Cargo manifests discovered under the root, in stable
    /// order. A root `Cargo.toml` yields a single entry; otherwise every
    /// non-ignored first-level `<dir>/Cargo.toml`.
    pub(crate) manifests: Vec<String>,
    /// The repo-relative `Cargo.lock` files that exist beside the discovered
    /// manifests. Authored into each task's `shared_inputs` so a lockfile change
    /// invalidates the cache; a workspace without a lockfile contributes none.
    #[serde(default)]
    pub(crate) lockfiles: Vec<String>,
    /// Whether any discovered workspace carries a `.config/nextest.toml`.
    pub(crate) nextest: bool,
}

impl RustFacts {
    /// Decode the facts from a [`Detection`]'s opaque table.
    ///
    /// # Errors
    /// Returns an error if the facts table is not the shape this adapter wrote.
    pub(crate) fn from_detection(detection: &Detection) -> AppResult<Self> {
        detection.facts.clone().try_into().map_err(|error| {
            AppError::new(ErrorCode::Internal, "invalid rust detection facts").with_cause(error)
        })
    }

    /// Encode the facts into an opaque [`Table`] for a [`Detection`].
    ///
    /// # Errors
    /// Returns an error only if the facts cannot be encoded as TOML.
    fn to_table(&self) -> AppResult<Table> {
        Table::try_from(self).map_err(|error| {
            AppError::new(ErrorCode::Internal, "encode rust facts").with_cause(error)
        })
    }
}

/// Detect the Cargo workspace(s) under `project_root` and, if any manifest is
/// found, return a [`Detection`] carrying the probed facts. Returns `None` when
/// neither a root nor a first-level nested `Cargo.toml` exists.
///
/// # Errors
/// Propagates a path-resolution, directory-listing, git-ignore, or
/// facts-encoding failure.
pub(crate) fn detect(project_root: &Path) -> AppResult<Option<Detection>> {
    let manifests = discover_manifests(project_root)?;
    if manifests.is_empty() {
        return Ok(None);
    }

    let lockfiles = existing_lockfiles(project_root, &manifests)?;
    let nextest = detect_nextest(project_root, &manifests);
    let facts = RustFacts {
        manifests,
        lockfiles,
        nextest,
    };
    let ecosystem = EcosystemId::new("rust")?;
    Ok(Some(Detection::new(ecosystem, facts.to_table()?)))
}

/// Whether any discovered workspace carries a `.config/nextest.toml`, marking the
/// repository as configured for `cargo-nextest`.
fn detect_nextest(project_root: &Path, manifests: &[String]) -> bool {
    manifests.iter().any(|manifest| {
        let dir = Path::new(manifest)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        safe_join(project_root, dir.join(NEXTEST_CONFIG)).is_ok_and(|path| path.is_file())
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use rskit_fs::TempDir;

    use super::{RustFacts, detect};

    fn write_manifest(dir: &Path, name: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "[package]\nname = \"x\"\n").unwrap();
    }

    #[test]
    fn absent_manifest_yields_none() {
        let dir = TempDir::new().unwrap();
        assert!(detect(dir.path()).unwrap().is_none());
    }

    #[test]
    fn root_manifest_is_discovered_alone() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "Cargo.toml");
        // A nested manifest is ignored once a root manifest is present.
        write_manifest(dir.path(), "core/Cargo.toml");

        let detection = detect(dir.path()).unwrap().expect("detection");
        assert_eq!(detection.ecosystem.as_str(), "rust");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert_eq!(facts.manifests, ["Cargo.toml"]);
        assert!(!facts.nextest);
    }

    #[test]
    fn nested_first_level_manifests_are_discovered_sorted() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "examples/Cargo.toml");
        write_manifest(dir.path(), "core/Cargo.toml");
        write_manifest(dir.path(), "contrib/Cargo.toml");
        // A hidden directory is skipped.
        write_manifest(dir.path(), ".hidden/Cargo.toml");

        let detection = detect(dir.path()).unwrap().expect("detection");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert_eq!(
            facts.manifests,
            [
                "contrib/Cargo.toml",
                "core/Cargo.toml",
                "examples/Cargo.toml"
            ]
        );
    }

    #[test]
    fn existing_lockfiles_are_recorded_per_workspace() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "core/Cargo.toml");
        fs::write(dir.path().join("core/Cargo.lock"), "# lock\n").unwrap();
        // A workspace without a lockfile contributes none.
        write_manifest(dir.path(), "contrib/Cargo.toml");

        let detection = detect(dir.path()).unwrap().expect("detection");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert_eq!(facts.lockfiles, ["core/Cargo.lock"]);
    }

    #[test]
    fn nextest_config_in_a_nested_workspace_is_detected() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "core/Cargo.toml");
        fs::create_dir_all(dir.path().join("core/.config")).unwrap();
        fs::write(dir.path().join("core/.config/nextest.toml"), "").unwrap();

        let detection = detect(dir.path()).unwrap().expect("detection");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert!(facts.nextest);
    }

    #[test]
    fn root_nextest_config_is_detected() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "Cargo.toml");
        fs::create_dir_all(dir.path().join(".config")).unwrap();
        fs::write(dir.path().join(".config/nextest.toml"), "").unwrap();

        let detection = detect(dir.path()).unwrap().expect("detection");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert!(facts.nextest);
    }
}
