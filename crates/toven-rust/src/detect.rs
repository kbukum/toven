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
use rskit_fs::sync_io::dir;
use rskit_git::IgnoreReader;
use rskit_git::cli::GitCli;
use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;
use toven_ports::Detection;

/// The manifest filename that marks a Cargo project or workspace root.
pub(crate) const ROOT_MANIFEST: &str = "Cargo.toml";

/// The nextest config that marks a workspace configured for `cargo-nextest`.
const NEXTEST_CONFIG: &str = ".config/nextest.toml";

/// The adapter-owned facts a Rust [`Detection`] carries to
/// [`render`](crate::render): the discovered manifests and whether any workspace
/// is configured for `cargo-nextest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RustFacts {
    /// The repo-relative Cargo manifests discovered under the root, in stable
    /// order. A root `Cargo.toml` yields a single entry; otherwise every
    /// non-ignored first-level `<dir>/Cargo.toml`.
    pub(crate) manifests: Vec<String>,
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

    let nextest = detect_nextest(project_root, &manifests);
    let facts = RustFacts { manifests, nextest };
    let ecosystem = EcosystemId::new("rust")?;
    Ok(Some(Detection::new(ecosystem, facts.to_table()?)))
}

/// Discover the repo-relative Cargo manifests under `project_root`.
///
/// A root `Cargo.toml` wins outright and is returned alone. Otherwise every
/// first-level subdirectory is scanned for a `<dir>/Cargo.toml`, skipping hidden
/// directories and any path ignored by Git, so a repository that groups several
/// workspaces under subdirectories is onboarded as one Rust ecosystem.
fn discover_manifests(project_root: &Path) -> AppResult<Vec<String>> {
    if manifest_exists(project_root, ROOT_MANIFEST)? {
        return Ok(vec![ROOT_MANIFEST.to_string()]);
    }

    let ignore = ignore_checker(project_root);
    let mut entries = dir::list(project_root)?;
    entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));

    let mut manifests = Vec::new();
    for entry in entries {
        if !entry.is_dir {
            continue;
        }
        let name = entry.file_name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let manifest = format!("{name}/{ROOT_MANIFEST}");
        if !manifest_exists(project_root, &manifest)? {
            continue;
        }
        if is_git_ignored(ignore.as_ref(), &manifest)? {
            continue;
        }
        manifests.push(manifest);
    }
    Ok(manifests)
}

/// Whether the repo-relative `manifest` resolves to a regular file under `root`.
fn manifest_exists(root: &Path, manifest: &str) -> AppResult<bool> {
    match safe_join(root, Path::new(manifest)) {
        Ok(path) => Ok(path.is_file()),
        Err(error) => Err(AppError::new(
            ErrorCode::Internal,
            format!("failed to resolve manifest path '{manifest}'"),
        )
        .with_cause(error)),
    }
}

/// A Git ignore checker rooted at `project_root`, or `None` when the root is not
/// inside a Git work tree (no ignore information is available, so nothing is
/// filtered).
fn ignore_checker(project_root: &Path) -> Option<GitCli> {
    rskit_git::discover(project_root)
        .ok()
        .map(|_| GitCli::new(project_root.to_path_buf()))
}

/// Whether `manifest` is ignored by Git. With no checker (not a Git repo) every
/// path is included; a genuine check-ignore failure inside a repo propagates.
fn is_git_ignored(checker: Option<&GitCli>, manifest: &str) -> AppResult<bool> {
    checker.map_or(Ok(false), |git| git.is_ignored(manifest))
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
