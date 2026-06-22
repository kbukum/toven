//! `CratesIoTarget` — the cargo/crates.io [`ReleaseTarget`] sliver.
//!
//! Step 4 stubs the **version I/O + manifest mutation** half (the part the
//! discovery/plan flow needs): read a module's declared version from its
//! `Cargo.toml`, and apply one atomic version mutation (own version + intra-
//! project dependency-floor rewrites) with format-preserving `toml_edit`. The
//! registry-facing half (`published_versions`, `package`, `publish`) lands in
//! step 9 against the real publish loop and returns a typed error until then —
//! never a success-shaped fallback.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::{exists, read_string_bounded, write_atomic_replace};
use rskit_version::semver::Version;
use toml_edit::{DocumentMut, Item, value};
use toven_model::Module;
use toven_ports::{Artifact, PublishOutcome, ReleaseMutation, ReleaseTarget};

/// Hard bound on a `Cargo.toml` read (4 MiB) — manifests are tiny; this only
/// guards against a pathological file.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for atomic manifest rewrites.
const MANIFEST_TEMP_PREFIX: &str = "toven-cargo-manifest";

/// The crates.io release target for the Rust ecosystem.
///
/// Resolves each module's repo-relative `manifest` against the process working
/// directory (the engine runs from the repository root).
#[derive(Debug, Default, Clone, Copy)]
pub struct CratesIoTarget;

impl CratesIoTarget {
    /// Construct the crates.io target.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Resolve a module's repo-relative manifest to an absolute path under the
    /// current working directory.
    fn manifest_path(module: &Module) -> AppResult<PathBuf> {
        let manifest = module.manifest.as_ref().ok_or_else(|| {
            AppError::invalid_input(
                "module.manifest",
                format!("module '{}' has no manifest to release", module.id),
            )
        })?;
        let cwd = Self::working_root()?;
        safe_join(&cwd, manifest.as_path()).map_err(|error| {
            AppError::invalid_input(
                "module.manifest",
                format!(
                    "manifest '{}' escapes the working directory: {error}",
                    manifest.as_path().display()
                ),
            )
        })
    }

    /// The trust boundary for manifest resolution: the process working directory
    /// (the engine runs from the repository root).
    fn working_root() -> AppResult<PathBuf> {
        std::env::current_dir().map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to read current directory").with_cause(error)
        })
    }
}

impl ReleaseTarget for CratesIoTarget {
    fn declared_version(&self, module: &Module) -> AppResult<Version> {
        let root = Self::working_root()?;
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        read_declared_version(&text, &path, &root)
    }

    fn published_versions(&self, _module: &Module) -> AppResult<Vec<Version>> {
        Err(deferred("published_versions"))
    }

    fn package(&self, _module: &Module) -> AppResult<Artifact> {
        Err(deferred("package"))
    }

    fn apply_release(&self, module: &Module, mutation: &ReleaseMutation) -> AppResult<()> {
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        let rewritten = apply_mutation(&text, mutation, &path)?;
        write_atomic_replace(&path, rewritten.as_bytes(), MANIFEST_TEMP_PREFIX)
    }

    fn publish(&self, _module: &Module, _artifact: &Artifact) -> AppResult<PublishOutcome> {
        Err(deferred("publish"))
    }
}

/// The typed error for the registry-facing methods deferred to step 9.
fn deferred(method: &str) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!("CratesIoTarget::{method} lands in step 9 (publish loop)"),
    )
}

/// Read `[package].version` from a `Cargo.toml` body.
///
/// A string `version` is returned directly. A workspace-inherited version
/// (`version.workspace = true`) is resolved from the nearest `[workspace.package]
/// version` — first in the same manifest (root package), then by walking ancestor
/// directories from `path` for the workspace-root `Cargo.toml`. The ancestor walk
/// never crosses above `root` (the working/repository-root trust boundary).
fn read_declared_version(text: &str, path: &Path, root: &Path) -> AppResult<Version> {
    let doc = parse_manifest(text, path)?;
    let version_item = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"));

    let missing = || {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("manifest '{}' has no [package].version", path.display()),
        )
    };

    let item = version_item.ok_or_else(missing)?;
    item.as_str().map_or_else(
        || {
            if is_workspace_inherited(item) {
                resolve_inherited_version(&doc, path, root)
            } else {
                Err(AppError::new(
                    ErrorCode::InvalidFormat,
                    format!(
                        "manifest '{}' has a [package].version that is neither a string nor \
                         `version.workspace = true`",
                        path.display()
                    ),
                ))
            }
        },
        |raw| parse_version(raw, path),
    )
}

/// Whether a `[package].version` entry is `version.workspace = true`.
fn is_workspace_inherited(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(Item::as_bool)
        == Some(true)
}

/// Resolve a `version.workspace = true` package version from the owning
/// workspace's `[workspace.package].version`.
///
/// The ancestor search is bounded to `root` (the working/repository-root trust
/// boundary): a `Cargo.toml` above `root` is never consulted, so resolution
/// cannot reach outside the repository and stays deterministic.
fn resolve_inherited_version(doc: &DocumentMut, path: &Path, root: &Path) -> AppResult<Version> {
    if let Some(raw) = workspace_package_version(doc) {
        return parse_version(raw, path);
    }

    let mut ancestor = path.parent().and_then(Path::parent);
    while let Some(dir) = ancestor {
        if !dir.starts_with(root) {
            break;
        }
        let candidate = dir.join("Cargo.toml");
        if candidate != path && exists(&candidate)? {
            let text = read_string_bounded(&candidate, MAX_MANIFEST_BYTES)?;
            let manifest = parse_manifest(&text, &candidate)?;
            if manifest.get("workspace").is_some() {
                return workspace_package_version(&manifest)
                    .ok_or_else(|| {
                        AppError::new(
                            ErrorCode::InvalidFormat,
                            format!(
                                "workspace root '{}' has no [workspace.package].version to inherit",
                                candidate.display()
                            ),
                        )
                    })
                    .and_then(|raw| parse_version(raw, &candidate));
            }
        }
        ancestor = dir.parent();
    }

    Err(AppError::new(
        ErrorCode::InvalidFormat,
        format!(
            "manifest '{}' declares version.workspace = true but no ancestor workspace root \
             with [workspace.package].version was found",
            path.display()
        ),
    ))
}

/// Read `[workspace.package].version` as a string, if present.
fn workspace_package_version(doc: &DocumentMut) -> Option<&str> {
    doc.get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("package"))
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"))
        .and_then(Item::as_str)
}

/// Parse a semantic version string, attributing failures to `path`.
fn parse_version(raw: &str, path: &Path) -> AppResult<Version> {
    Version::parse(raw).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("invalid version '{raw}' in '{}'", path.display()),
        )
        .with_cause(error)
    })
}

/// Apply one [`ReleaseMutation`] to a `Cargo.toml` body, returning the rewritten
/// text. Own version and each dependency floor are set with format-preserving
/// edits; the document is otherwise untouched.
fn apply_mutation(text: &str, mutation: &ReleaseMutation, path: &Path) -> AppResult<String> {
    let mut doc = parse_manifest(text, path)?;

    if let Some(new_version) = &mutation.new_version {
        let package = doc
            .get_mut("package")
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::InvalidFormat,
                    format!("manifest '{}' has no [package] table", path.display()),
                )
            })?;
        package.insert("version", value(new_version.to_string()));
    }

    for (dependency, floor) in &mutation.dep_floor_updates {
        set_dependency_floor(&mut doc, &dependency.name, floor);
    }

    Ok(doc.to_string())
}

/// Rewrite the version requirement of one intra-project dependency across every
/// dependency table (`[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`).
fn set_dependency_floor(doc: &mut DocumentMut, name: &str, floor: &Version) {
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = doc.get_mut(table_name).and_then(Item::as_table_like_mut) else {
            continue;
        };
        let Some(entry) = table.get_mut(name) else {
            continue;
        };
        match entry {
            Item::Value(existing) if existing.is_str() => {
                *entry = value(floor.to_string());
            }
            _ => {
                if let Some(detail) = entry.as_table_like_mut() {
                    // A workspace-inherited dependency (`{ workspace = true }`) must
                    // not gain a `version` key — cargo rejects the combination.
                    let inherited =
                        detail.get("workspace").and_then(toml_edit::Item::as_bool) == Some(true);
                    if !inherited {
                        detail.insert("version", value(floor.to_string()));
                    }
                }
            }
        }
    }
}

/// Parse a `Cargo.toml` body into an editable document.
fn parse_manifest(text: &str, path: &Path) -> AppResult<DocumentMut> {
    text.parse::<DocumentMut>().map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("failed to parse manifest '{}'", path.display()),
        )
        .with_cause(error)
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleRef};
    use toven_ports::ReleaseMutation;

    use toml_edit::Item;

    use super::{apply_mutation, read_declared_version};

    const MANIFEST: &str = "\
[package]
name = \"app\"
version = \"1.2.3\"

[dependencies]
core = { version = \"0.1.0\", path = \"../core\" }
plain = \"0.4.0\"
";

    /// A root manifest that is both the workspace root and a package, where the
    /// package inherits its version from `[workspace.package]` in the same file.
    const ROOT_INHERITED_MANIFEST: &str = "\
[workspace]
members = [\".\"]

[workspace.package]
version = \"3.4.5\"

[package]
name = \"app\"
version.workspace = true

[dependencies]
core = { workspace = true }
plain = \"0.4.0\"
";

    fn dep(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    #[test]
    fn reads_declared_version() {
        let version =
            read_declared_version(MANIFEST, Path::new("Cargo.toml"), Path::new("")).unwrap();
        assert_eq!(version, Version::new(1, 2, 3));
    }

    #[test]
    fn missing_version_is_rejected() {
        assert!(
            read_declared_version("[package]\nname = \"x\"\n", Path::new("C"), Path::new(""))
                .is_err()
        );
    }

    #[test]
    fn invalid_version_type_is_distinguished_from_missing() {
        // `version` is present but neither a string nor `version.workspace = true`.
        let manifest = "[package]\nname = \"x\"\nversion = 1\n";
        let error = read_declared_version(manifest, Path::new("C"), Path::new("")).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("neither a string nor"),
            "expected an invalid-type message, got: {message}"
        );
        assert!(
            !message.contains("has no [package].version"),
            "invalid type must not report a missing version: {message}"
        );
    }

    #[test]
    fn resolves_version_inherited_from_workspace_in_same_manifest() {
        let version =
            read_declared_version(ROOT_INHERITED_MANIFEST, Path::new("Cargo.toml"), Path::new(""))
                .unwrap();
        assert_eq!(version, Version::new(3, 4, 5));
    }

    #[test]
    fn inherited_version_without_a_resolvable_workspace_root_is_rejected() {
        // `version.workspace = true` but no `[workspace.package].version` reachable
        // (the manifest has no inline workspace table and the path has no ancestors).
        let manifest = "[package]\nname = \"x\"\nversion.workspace = true\n";
        assert!(read_declared_version(manifest, Path::new("Cargo.toml"), Path::new("")).is_err());
    }

    #[test]
    fn dep_floor_skips_workspace_inherited_dependencies() {
        let mut mutation = ReleaseMutation::version(Version::new(2, 0, 0));
        // `core` is workspace-inherited; bumping its floor must not stamp a
        // `version` key onto it (cargo forbids `workspace = true` + `version`).
        mutation
            .dep_floor_updates
            .insert(dep("core"), Version::new(0, 2, 0));

        let rewritten =
            apply_mutation(ROOT_INHERITED_MANIFEST, &mutation, Path::new("Cargo.toml")).unwrap();

        let doc = rewritten
            .parse::<toml_edit::DocumentMut>()
            .expect("valid toml");
        let core = doc["dependencies"]["core"]
            .as_table_like()
            .expect("core is a table");
        assert_eq!(core.get("workspace").and_then(Item::as_bool), Some(true));
        assert!(
            core.get("version").is_none(),
            "no version stamped: {rewritten}"
        );
    }

    #[test]
    fn applies_own_version_and_dep_floors() {
        let mut mutation = ReleaseMutation::version(Version::new(2, 0, 0));
        mutation
            .dep_floor_updates
            .insert(dep("core"), Version::new(0, 2, 0));
        mutation
            .dep_floor_updates
            .insert(dep("plain"), Version::new(0, 5, 0));

        let rewritten = apply_mutation(MANIFEST, &mutation, Path::new("Cargo.toml")).unwrap();

        assert!(rewritten.contains("version = \"2.0.0\""));
        assert!(rewritten.contains("core = { version = \"0.2.0\""));
        assert!(rewritten.contains("path = \"../core\""));
        assert!(rewritten.contains("plain = \"0.5.0\""));
    }

    #[test]
    fn unknown_dep_floor_is_ignored() {
        let mut mutation = ReleaseMutation::version(Version::new(2, 0, 0));
        mutation
            .dep_floor_updates
            .insert(dep("absent"), Version::new(9, 9, 9));
        let rewritten = apply_mutation(MANIFEST, &mutation, Path::new("Cargo.toml")).unwrap();
        assert!(!rewritten.contains("9.9.9"));
    }
}
