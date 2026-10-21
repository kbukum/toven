//! Cargo manifest versions: read a module's declared version (own or
//! workspace-inherited) and apply one release mutation to the manifest that
//! owns it.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::file::{exists, read_string_bounded, write_atomic_replace};
use rskit_version::semver::Version;
use toml_edit::{DocumentMut, Item, value};
use toven_model::{Module, RepoPath};
use toven_ports::{ManifestMutator, ReleaseMutation};

use super::target::CargoRegistryTarget;

/// Hard bound on a `Cargo.toml` read (4 MiB) — manifests are tiny; this only
/// guards against a pathological file.
pub(super) const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for atomic manifest rewrites.
const MANIFEST_TEMP_PREFIX: &str = "toven-cargo-manifest";

impl CargoRegistryTarget {
    /// Record the version a workspace-root manifest is being asked to bump to,
    /// failing closed on a conflicting request.
    ///
    /// A single `release apply` invokes [`Self::apply_release`] once per member;
    /// every inheriting member of one workspace routes its version bump to the
    /// same root. The first request records `owner → version`; a later request
    /// for the **same** root and the **same** version is idempotent, but a
    /// request for a **different** version is a real conflict (a per-sibling
    /// bump divergence such as minor vs patch) that would otherwise silently
    /// last-writer-wins the shared root while earlier siblings tagged the other
    /// version. Reject it with an actionable typed error rather than corrupting
    /// the release.
    ///
    /// # Errors
    /// A typed conflict error when `owner` was already recorded at a different
    /// version, or an internal error if the shared lock is poisoned.
    fn record_workspace_version(&self, owner: &Path, new_version: &Version) -> AppResult<()> {
        let mut recorded = self.workspace_root_versions.lock().map_err(|_| {
            AppError::new(
                ErrorCode::Internal,
                "workspace-root version ledger lock was poisoned",
            )
        })?;
        if let Some(existing) = recorded.get(owner) {
            if existing == new_version {
                return Ok(());
            }
            let existing = existing.clone();
            drop(recorded);
            return Err(AppError::invalid_input(
                "release.version",
                format!(
                    "workspace root '{}' received divergent version bumps ('{existing}' and \
                     '{new_version}'); its members share one inherited \
                     `[workspace.package].version`, so release them lock-step with a single \
                     `--set-version` rather than per-member cascade levels",
                    owner.display()
                ),
            ));
        }
        recorded.insert(owner.to_path_buf(), new_version.clone());
        drop(recorded);
        Ok(())
    }
}

impl ManifestMutator for CargoRegistryTarget {
    fn apply_release(
        &self,
        module: &Module,
        mutation: &ReleaseMutation,
    ) -> AppResult<Vec<RepoPath>> {
        let manifest = module.manifest.clone().ok_or_else(|| {
            AppError::invalid_input(
                "module.manifest",
                format!("module '{}' has no manifest to release", module.id),
            )
        })?;
        let working_root = Self::working_root()?;
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        let mut doc = parse_manifest(&text, &path)?;

        // A version bump routes to whichever manifest actually *owns* the
        // version: the member's own `[package].version` when it declares a
        // literal, or the workspace root's `[workspace.package].version` when
        // the member inherits it (`version.workspace = true`). Resolving this
        // before mutating keeps the writer's source of truth identical to the
        // reader's (`read_declared_version`), so `release plan`/`status` and the
        // applied mutation can never diverge.
        let version_site = match &mutation.new_version {
            Some(_) => Some(resolve_version_site(&doc, &path, &working_root)?),
            None => None,
        };

        // Record/validate the workspace-root version conflict FIRST, before any
        // modification or disk write so a divergent request fails closed without
        // partial state.
        if let (Some(new_version), Some(VersionSite::Workspace(owner))) =
            (&mutation.new_version, version_site.as_ref())
        {
            self.record_workspace_version(owner, new_version)?;
        }

        // Dependency-floor rewrites are always per-member (`[dependencies]` &c.).
        for (dependency, floor) in &mutation.dep_floor_updates {
            set_dependency_floor(&mut doc, &dependency.name, floor);
        }

        if let (Some(new_version), Some(site)) = (&mutation.new_version, version_site.as_ref()) {
            match site {
                // Literal member version: bump in place.
                VersionSite::OwnPackage => set_package_version(&mut doc, new_version, &path)?,
                // A root that is both workspace and package (the inherited
                // version lives in this same manifest): bump in place.
                VersionSite::Workspace(owner) if owner == &path => {
                    set_workspace_package_version(&mut doc, new_version, &path)?;
                }
                // Inherited from a *separate* workspace-root manifest: this
                // member's version is not written here at all.
                VersionSite::Workspace(_) => {}
            }
        }

        let rewritten = doc.to_string();
        let path_changed = rewritten != text;

        // Route an inherited version bump to the owning workspace-root manifest
        // exactly once. Many members of a single-version workspace resolve to
        // the same root; the owner path is staged only when a write actually
        // landed — an already-at-target root is a no-op returning no path (the
        // engine still dedupes across members as a backstop).
        let owner_change = if let (Some(new_version), Some(VersionSite::Workspace(owner))) =
            (&mutation.new_version, version_site.as_ref())
            && owner != &path
        {
            let owner_text = read_string_bounded(owner, MAX_MANIFEST_BYTES)?;
            let owner_rewritten = apply_workspace_root_version(&owner_text, owner, new_version)?;
            (owner_rewritten != owner_text).then(|| (owner.clone(), owner_rewritten))
        } else {
            None
        };

        // Perform disk writes after all in-memory validation and preparation.
        // The mutation can span two files (a member's dependency floors plus a
        // shared workspace-root version); keep it transactional: if the owner
        // write fails after the member write landed, roll the member back so an
        // `Err` never leaves partial state on disk.
        let mut changed = Vec::new();
        if path_changed {
            write_atomic_replace(&path, rewritten.as_bytes(), MANIFEST_TEMP_PREFIX)?;
            changed.push(manifest);
        }

        if let Some((owner_path, owner_rewritten)) = owner_change {
            if let Err(error) = write_atomic_replace(
                &owner_path,
                owner_rewritten.as_bytes(),
                MANIFEST_TEMP_PREFIX,
            ) {
                if path_changed {
                    write_atomic_replace(&path, text.as_bytes(), MANIFEST_TEMP_PREFIX).map_err(
                        |rollback| {
                            AppError::new(
                                ErrorCode::Internal,
                                format!(
                                    "workspace-root write failed and rolling back '{}' failed \
                                     too; the member manifest may be partially updated",
                                    path.display()
                                ),
                            )
                            .with_cause(rollback)
                        },
                    )?;
                }
                return Err(error);
            }
            changed.push(workspace_root_repo_path(&owner_path, &working_root)?);
        }

        Ok(changed)
    }
}

/// Parse `[package].version` from a `Cargo.toml` **body** without touching the
/// working tree, for anchoring a module on its own version at a historical
/// commit (an umbrella-tag baseline).
///
/// Returns `None` — never an error — when the body is well-formed TOML but does
/// not resolve a concrete version here: no `[package].version` (a virtual
/// workspace manifest), or a `version.workspace = true` whose owning
/// `[workspace.package].version` lives in an ancestor manifest not available
/// from this body alone. The caller falls back to the umbrella tag's own
/// version in those cases. Malformed TOML or a non-string, non-inherited
/// version is a genuine parse fault and propagates.
pub(super) fn version_in_manifest_contents(text: &str) -> AppResult<Option<Version>> {
    let attribution = Path::new("Cargo.toml");
    let doc = parse_manifest(text, attribution)?;
    let Some(item) = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"))
    else {
        return Ok(None);
    };
    match item.as_str() {
        Some(raw) => parse_version(raw, attribution).map(Some),
        // `version.workspace = true`: the concrete version is in the workspace
        // root manifest at the same commit, not this body — fall back rather
        // than resolve a working-tree ancestor that may differ from the commit.
        None if is_workspace_inherited(item) => Ok(None),
        None => Err(AppError::new(
            ErrorCode::InvalidFormat,
            "manifest has a [package].version that is neither a string nor \
             `version.workspace = true`",
        )),
    }
}

/// `[workspace.package] version` — first in the same manifest (root package),
/// then by walking ancestor directories from `path` for the workspace-root
/// `Cargo.toml`. The ancestor walk never crosses above `root` (the
/// working/repository-root trust boundary).
pub(super) fn read_declared_version(text: &str, path: &Path, root: &Path) -> AppResult<Version> {
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
/// Reader/writer share [`resolve_workspace_root_manifest`] so the version the
/// reader reports and the manifest the writer bumps are always the same source
/// of truth.
fn resolve_inherited_version(doc: &DocumentMut, path: &Path, root: &Path) -> AppResult<Version> {
    let (owner_path, owner_doc) = resolve_workspace_root_manifest(doc, path, root)?;
    let raw = workspace_package_version(&owner_doc).ok_or_else(|| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!(
                "workspace root '{}' has no [workspace.package].version to inherit",
                owner_path.display()
            ),
        )
    })?;
    parse_version(raw, &owner_path)
}

/// Where a member's version bump must be written.
#[derive(Debug, Clone, Eq, PartialEq)]
enum VersionSite {
    /// The member manifest's own `[package].version` (a literal version).
    OwnPackage,
    /// `[workspace.package].version` of the owning workspace-root manifest at
    /// this path (an inherited `version.workspace = true`). The path equals the
    /// member manifest when the root is both a workspace and a package.
    Workspace(PathBuf),
}

/// Decide where a member's version bump lands: its own `[package].version`
/// (literal) or the `[workspace.package].version` of the owning workspace root
/// (`version.workspace = true`).
///
/// This mirrors [`read_declared_version`]'s classification exactly, so the
/// mutation always targets the same manifest the reader resolves the version
/// from. A `[package]` with no `version` key falls back to writing an own
/// literal (a package cargo would otherwise reject).
fn resolve_version_site(doc: &DocumentMut, path: &Path, root: &Path) -> AppResult<VersionSite> {
    let version_item = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"));

    match version_item {
        Some(item) if item.as_str().is_some() => Ok(VersionSite::OwnPackage),
        Some(item) if is_workspace_inherited(item) => {
            let (owner_path, _) = resolve_workspace_root_manifest(doc, path, root)?;
            Ok(VersionSite::Workspace(owner_path))
        }
        Some(_) => Err(AppError::new(
            ErrorCode::InvalidFormat,
            format!(
                "manifest '{}' has a [package].version that is neither a string nor \
                 `version.workspace = true`",
                path.display()
            ),
        )),
        None => Ok(VersionSite::OwnPackage),
    }
}

/// Locate the manifest whose `[workspace.package]` backs a member's
/// `version.workspace = true`, returning its path and parsed document.
///
/// The version may live inline in the member manifest (a root that is both a
/// workspace and a package) or in an ancestor `Cargo.toml`. The ancestor search
/// is bounded to `root` (the working/repository-root trust boundary): a
/// `Cargo.toml` above `root` is never consulted, so resolution cannot reach
/// outside the repository and stays deterministic.
fn resolve_workspace_root_manifest(
    doc: &DocumentMut,
    path: &Path,
    root: &Path,
) -> AppResult<(PathBuf, DocumentMut)> {
    if workspace_package_version(doc).is_some() {
        return Ok((path.to_path_buf(), doc.clone()));
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
                if workspace_package_version(&manifest).is_some() {
                    return Ok((candidate, manifest));
                }
                return Err(AppError::new(
                    ErrorCode::InvalidFormat,
                    format!(
                        "workspace root '{}' has no [workspace.package].version to inherit",
                        candidate.display()
                    ),
                ));
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

/// Set a member's own `[package].version` to a literal string.
fn set_package_version(doc: &mut DocumentMut, version: &Version, path: &Path) -> AppResult<()> {
    let package = doc
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!("manifest '{}' has no [package] table", path.display()),
            )
        })?;
    package.insert("version", value(version.to_string()));
    Ok(())
}

/// Set `[workspace.package].version` to a literal string.
fn set_workspace_package_version(
    doc: &mut DocumentMut,
    version: &Version,
    path: &Path,
) -> AppResult<()> {
    let workspace = doc
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!(
                    "workspace root '{}' has no [workspace] table",
                    path.display()
                ),
            )
        })?;
    let package = workspace
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!(
                    "workspace root '{}' has no [workspace.package] table to bump",
                    path.display()
                ),
            )
        })?;
    package.insert("version", value(version.to_string()));
    Ok(())
}

/// Rewrite a workspace-root manifest's `[workspace.package].version`, returning
/// the format-preserving rewritten body. Used when a version bump must land on
/// a manifest *separate* from the member being released.
fn apply_workspace_root_version(text: &str, path: &Path, version: &Version) -> AppResult<String> {
    let mut doc = parse_manifest(text, path)?;
    set_workspace_package_version(&mut doc, version, path)?;
    Ok(doc.to_string())
}

/// Render a workspace-root manifest's absolute path as a repo-relative
/// [`RepoPath`], failing closed if it escapes the working root.
fn workspace_root_repo_path(owner: &Path, working_root: &Path) -> AppResult<RepoPath> {
    let relative = owner.strip_prefix(working_root).map_err(|_| {
        AppError::invalid_input(
            "release.workspace_root",
            format!(
                "workspace root manifest '{}' is outside the working root '{}'",
                owner.display(),
                working_root.display()
            ),
        )
    })?;
    RepoPath::new(relative)
}

/// Rewrite the version requirement of one intra-project dependency across every
/// dependency table (`[dependencies]`, `[dev-dependencies]`,
/// `[build-dependencies]`).
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
                    // A workspace-inherited dependency (`{ workspace = true }`) must not gain a
                    // `version` key — cargo rejects the combination.
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
    use toml_edit::Item;

    use super::{
        apply_workspace_root_version, parse_manifest, read_declared_version, set_dependency_floor,
        set_package_version, set_workspace_package_version, version_in_manifest_contents,
    };

    const MANIFEST: &str = "\
[package]
name = \"app\"
version = \"1.2.3\"

[dependencies]
core = { version = \"0.1.0\", path = \"../core\" }
plain = \"0.4.0\"
";

    /// A root manifest that is both the workspace root and a package, where the
    /// package inherits its version from `[workspace.package]` in the same
    /// file.
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
        let version = read_declared_version(
            ROOT_INHERITED_MANIFEST,
            Path::new("Cargo.toml"),
            Path::new(""),
        )
        .unwrap();
        assert_eq!(version, Version::new(3, 4, 5));
    }

    #[test]
    fn version_in_manifest_reads_an_explicit_package_version() {
        let version = version_in_manifest_contents(MANIFEST).unwrap();
        assert_eq!(version, Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn version_in_manifest_is_none_for_a_workspace_inherited_version() {
        assert_eq!(
            version_in_manifest_contents(ROOT_INHERITED_MANIFEST).unwrap(),
            None
        );
    }

    #[test]
    fn version_in_manifest_is_none_when_no_package_version_is_declared() {
        assert_eq!(
            version_in_manifest_contents("[package]\nname = \"x\"\n").unwrap(),
            None
        );
    }

    #[test]
    fn version_in_manifest_rejects_a_non_string_non_inherited_version() {
        assert!(version_in_manifest_contents("[package]\nname = \"x\"\nversion = 1\n").is_err());
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
        // `core` is workspace-inherited; bumping its floor must not stamp a `version`
        // key onto it (cargo forbids `workspace = true` + `version`).
        let mut doc = parse_manifest(ROOT_INHERITED_MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_dependency_floor(&mut doc, "core", &Version::new(0, 2, 0));

        let core = doc["dependencies"]["core"]
            .as_table_like()
            .expect("core is a table");
        assert_eq!(core.get("workspace").and_then(Item::as_bool), Some(true));
        assert!(core.get("version").is_none(), "no version stamped");
    }

    #[test]
    fn applies_own_version_and_dep_floors() {
        let mut doc = parse_manifest(MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_package_version(&mut doc, &Version::new(2, 0, 0), Path::new("Cargo.toml")).unwrap();
        set_dependency_floor(&mut doc, "core", &Version::new(0, 2, 0));
        set_dependency_floor(&mut doc, "plain", &Version::new(0, 5, 0));
        let rewritten = doc.to_string();

        assert!(rewritten.contains("version = \"2.0.0\""));
        assert!(rewritten.contains("core = { version = \"0.2.0\""));
        assert!(rewritten.contains("path = \"../core\""));
        assert!(rewritten.contains("plain = \"0.5.0\""));
    }

    #[test]
    fn unknown_dep_floor_is_ignored() {
        let mut doc = parse_manifest(MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_dependency_floor(&mut doc, "absent", &Version::new(9, 9, 9));
        assert!(!doc.to_string().contains("9.9.9"));
    }

    #[test]
    fn set_workspace_package_version_bumps_the_shared_version_in_place() {
        // A root that is both workspace and package: the bump lands on
        // `[workspace.package].version`, and the member `[package]` keeps its
        // `version.workspace = true` inheritance untouched.
        let mut doc = parse_manifest(ROOT_INHERITED_MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_workspace_package_version(&mut doc, &Version::new(9, 9, 9), Path::new("Cargo.toml"))
            .unwrap();
        let rewritten = doc.to_string();

        assert!(
            rewritten.contains("[workspace.package]\nversion = \"9.9.9\""),
            "workspace version bumped: {rewritten}"
        );
        assert!(
            rewritten.contains("version.workspace = true"),
            "member inheritance preserved: {rewritten}"
        );
        assert!(
            !rewritten.contains("[package]\nname = \"app\"\nversion = \"9.9.9\""),
            "member [package] must not gain a literal version: {rewritten}"
        );
    }

    #[test]
    fn apply_workspace_root_version_rewrites_only_the_workspace_version() {
        // A virtual workspace root (no `[package]`): the bump lands on
        // `[workspace.package].version` alone.
        let root = "\
[workspace]
members = [\"crates/app\"]

[workspace.package]
version = \"0.3.0\"
";
        let rewritten =
            apply_workspace_root_version(root, Path::new("Cargo.toml"), &Version::new(0, 4, 0))
                .unwrap();
        assert!(rewritten.contains("version = \"0.4.0\""));
        assert!(!rewritten.contains("0.3.0"));
    }

    #[test]
    fn apply_workspace_root_version_rejects_a_root_without_a_workspace_package_table() {
        let root = "[workspace]\nmembers = [\"crates/app\"]\n";
        assert!(
            apply_workspace_root_version(root, Path::new("Cargo.toml"), &Version::new(0, 4, 0))
                .is_err()
        );
    }
}
