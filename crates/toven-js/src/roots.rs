//! JavaScript roots and their workspace members.
//!
//! A root is a directory with its own `package.json` and its own install.
//! Under `roots = "auto"` the repository-root `package.json` is always a root;
//! a nested `package.json` selected by its nearest enclosing root's workspace
//! patterns is that root's member; an unselected nested `package.json`
//! becomes a root of its own only when it carries a root marker (a lockfile, a
//! workspace definition, or a `packageManager` pin) and is otherwise ignored.
//! A `package.json` outside every root is always a root. An explicit `roots`
//! list freezes the root set; a listed root that another root's patterns also
//! select is a conflict.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file;

use crate::config::{JsConfig, Roots};
use crate::manifest::{
    PACKAGE_JSON, PNPM_WORKSPACE, PackageJson, read_package_json, read_pnpm_workspace,
};
use crate::package_manager::{self, Manager};
use crate::patterns::MemberPatterns;
use crate::walk;

/// One JavaScript root with its detected package manager and members.
#[derive(Debug)]
pub(crate) struct Root {
    /// The repo-relative root directory (empty for the project root).
    pub(crate) dir: PathBuf,
    /// The root `package.json`.
    pub(crate) manifest: PackageJson,
    /// The root's package manager.
    pub(crate) manager: Manager,
    /// The version the root's `packageManager` pins for that manager.
    pub(crate) pinned: Option<String>,
    /// The root's workspace member patterns.
    patterns: MemberPatterns,
    /// The repo-relative member directories, sorted.
    pub(crate) members: Vec<PathBuf>,
}

impl Root {
    /// Whether the root declares workspace members.
    pub(crate) const fn is_workspace(&self) -> bool {
        !self.patterns.is_empty()
    }
}

/// Resolve every JavaScript root under `project_root` with its members.
///
/// # Errors
/// An explicit root that escapes the project or is missing, a malformed
/// manifest, an ambiguous package manager, a root that is also a member, or a
/// `package_manager` or `pack` entry keyed by a directory that is not a root.
pub(crate) fn resolve(config: &JsConfig, project_root: &Path) -> AppResult<Vec<Root>> {
    let explicit = match &config.roots {
        Roots::Auto => None,
        Roots::Explicit(manifests) => Some(explicit_dirs(project_root, manifests)?),
    };
    let mut dirs: BTreeSet<PathBuf> = walk::package_dirs(project_root, &config.exclude)?
        .into_iter()
        .collect();
    if let Some(explicit) = &explicit {
        dirs.extend(explicit.iter().cloned());
    }

    let mut roots: Vec<Root> = Vec::new();
    for dir in dirs {
        let nearest = roots
            .iter()
            .enumerate()
            .filter(|(_, root)| dir != root.dir && dir.starts_with(&root.dir))
            .max_by_key(|(_, root)| root.dir.components().count())
            .map(|(index, _)| index);
        let owner = nearest.filter(|&index| {
            let root = &roots[index];
            dir.strip_prefix(&root.dir)
                .is_ok_and(|relative| root.patterns.matches(relative))
        });
        let is_root = match &explicit {
            Some(explicit) => {
                if !explicit.contains(&dir) {
                    false
                } else if let Some(owner) = owner {
                    return Err(AppError::new(
                        ErrorCode::Conflict,
                        format!(
                            "JavaScript root '{}' is also a workspace member of root '{}'; remove \
                             it from [ecosystems.js] roots or from that root's workspaces",
                            label(&dir),
                            label(&roots[owner].dir)
                        ),
                    ));
                } else {
                    true
                }
            }
            None => owner.is_none() && (nearest.is_none() || has_marker(project_root, &dir)?),
        };
        if is_root {
            roots.push(open_root(config, project_root, dir)?);
        } else if let Some(owner) = owner {
            roots[owner].members.push(dir);
        }
    }
    check_root_keys(config, &roots)?;
    Ok(roots)
}

/// Reject a root-keyed setting (`package_manager`, `pack`) whose key is not a
/// resolved root, so a typo fails loudly instead of being silently ignored.
fn check_root_keys(config: &JsConfig, roots: &[Root]) -> AppResult<()> {
    let keyed = config
        .package_manager
        .keys()
        .map(|key| ("package_manager", key))
        .chain(config.pack.keys().map(|key| ("pack", key)));
    for (setting, key) in keyed {
        if !roots.iter().any(|root| label(&root.dir) == *key) {
            let known: Vec<String> = roots.iter().map(|root| label(&root.dir)).collect();
            return Err(AppError::invalid_input(
                format!("ecosystems.js.{setting}.{key}"),
                format!(
                    "'{key}' is not a JavaScript root (roots: {})",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ),
            ));
        }
    }
    Ok(())
}

/// The repo-relative label of a directory (`.` for the project root).
pub(crate) fn label(dir: &Path) -> String {
    if dir.as_os_str().is_empty() {
        ".".to_string()
    } else {
        dir.components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// The repo-relative `package.json` label of a directory.
pub(crate) fn manifest_label(dir: &Path) -> String {
    if dir.as_os_str().is_empty() {
        PACKAGE_JSON.to_string()
    } else {
        format!("{}/{PACKAGE_JSON}", label(dir))
    }
}

/// Read the `package.json` in the repo-relative `dir`.
pub(crate) fn read_manifest(project_root: &Path, dir: &Path) -> AppResult<PackageJson> {
    read_package_json(
        &project_root.join(dir).join(PACKAGE_JSON),
        &manifest_label(dir),
    )
}

/// Validate the explicit root manifests into repo-relative directories.
fn explicit_dirs(project_root: &Path, manifests: &[String]) -> AppResult<BTreeSet<PathBuf>> {
    let mut dirs = BTreeSet::new();
    for manifest in manifests {
        let escape = || {
            AppError::invalid_input(
                "ecosystems.js.roots",
                format!("root '{manifest}' must be a package.json path inside the project"),
            )
        };
        let mut relative = PathBuf::new();
        for component in Path::new(manifest).components() {
            match component {
                Component::Normal(part) => relative.push(part),
                Component::CurDir => {}
                _ => return Err(escape()),
            }
        }
        if relative.file_name() != Some(PACKAGE_JSON.as_ref()) {
            return Err(escape());
        }
        let absolute =
            safe_join(project_root, &relative).map_err(|error| escape().with_cause(error))?;
        if !file::exists(&absolute)? {
            return Err(AppError::new(
                ErrorCode::NotFound,
                format!("root '{manifest}' listed in [ecosystems.js] roots does not exist"),
            ));
        }
        dirs.insert(relative.parent().map(Path::to_path_buf).unwrap_or_default());
    }
    Ok(dirs)
}

/// Whether the nested `package.json` directory carries a root marker.
fn has_marker(project_root: &Path, dir: &Path) -> AppResult<bool> {
    let absolute = project_root.join(dir);
    if !package_manager::lockfiles_in(&absolute)?.is_empty()
        || file::exists(&absolute.join(PNPM_WORKSPACE))?
    {
        return Ok(true);
    }
    let manifest = read_manifest(project_root, dir)?;
    Ok(manifest.declares_workspaces() || manifest.package_manager.is_some())
}

/// Read a root's manifest, detect its package manager, and parse its member
/// patterns: pnpm reads `pnpm-workspace.yaml`, every other manager reads the
/// `workspaces` field.
fn open_root(config: &JsConfig, project_root: &Path, dir: PathBuf) -> AppResult<Root> {
    let absolute = project_root.join(&dir);
    let manifest = read_manifest(project_root, &dir)?;
    let override_tool = config.package_manager.get(&label(&dir)).copied();
    let manager = package_manager::detect(&absolute, &label(&dir), &manifest, override_tool)?;
    let pinned = package_manager::pinned_version(&label(&dir), &manifest, manager)?;
    let patterns = if manager == Manager::Pnpm {
        let yaml = absolute.join(PNPM_WORKSPACE);
        let source = if dir.as_os_str().is_empty() {
            PNPM_WORKSPACE.to_string()
        } else {
            format!("{}/{PNPM_WORKSPACE}", label(&dir))
        };
        if file::exists(&yaml)? {
            MemberPatterns::parse(&source, &read_pnpm_workspace(&yaml, &source)?.packages)?
        } else {
            MemberPatterns::default()
        }
    } else {
        MemberPatterns::parse(&manifest_label(&dir), manifest.workspace_patterns())?
    };
    Ok(Root {
        dir,
        manifest,
        manager,
        pinned,
        patterns,
        members: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rskit_errors::ErrorCode;

    use super::{Root, resolve};
    use crate::config::{JsConfig, Roots};
    use crate::package_manager::{Manager, Tool};

    fn tree(files: &[(&str, &str)]) -> rskit_fs::TempDir {
        let dir = rskit_fs::TempDir::new().expect("tempdir");
        for (file, text) in files {
            dir.write_file(file, text.as_bytes()).expect("write");
        }
        dir
    }

    fn summary(roots: &[Root]) -> Vec<(String, Vec<PathBuf>)> {
        roots
            .iter()
            .map(|root| (super::label(&root.dir), root.members.clone()))
            .collect()
    }

    fn auto(dir: &Path) -> Vec<Root> {
        resolve(&JsConfig::default(), dir).expect("roots")
    }

    #[test]
    fn auto_splits_members_nested_roots_and_ignored_packages() {
        let dir = tree(&[
            ("package.json", r#"{"workspaces":["packages/*"]}"#),
            ("bun.lock", ""),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            (
                "packages/b/package.json",
                r#"{"name":"b","workspaces":["x/*"]}"#,
            ),
            ("web/package.json", r#"{"name":"web"}"#),
            ("web/bun.lock", ""),
            ("test/fixture/package.json", r#"{"name":"fixture"}"#),
        ]);
        let roots = auto(dir.path());
        assert_eq!(
            summary(&roots),
            [
                (
                    ".".to_string(),
                    vec![PathBuf::from("packages/a"), PathBuf::from("packages/b")]
                ),
                ("web".to_string(), vec![]),
            ]
        );
        assert_eq!(roots[0].manager, Manager::Bun);
        assert!(roots[0].is_workspace());
        assert!(!roots[1].is_workspace());
    }

    #[test]
    fn packages_outside_every_root_are_roots() {
        let dir = tree(&[
            ("gen/ts/package.json", r#"{"name":"gen"}"#),
            ("web/package.json", r#"{"name":"web"}"#),
        ]);
        assert_eq!(
            summary(&auto(dir.path())),
            [("gen/ts".to_string(), vec![]), ("web".to_string(), vec![])]
        );
    }

    #[test]
    fn pnpm_reads_members_from_its_workspace_file() {
        let dir = tree(&[
            ("package.json", r#"{"workspaces":["ignored/*"]}"#),
            ("pnpm-lock.yaml", ""),
            (
                "pnpm-workspace.yaml",
                "packages:\n  - 'libs/*'\n  - '!libs/skip'\n",
            ),
            ("libs/a/package.json", r#"{"name":"a"}"#),
            ("libs/skip/package.json", r#"{"name":"skip"}"#),
            ("ignored/x/package.json", r#"{"name":"x"}"#),
        ]);
        let roots = auto(dir.path());
        assert_eq!(roots[0].manager, Manager::Pnpm);
        assert_eq!(roots[0].members, [PathBuf::from("libs/a")]);
    }

    #[test]
    fn explicit_roots_freeze_the_set_and_reject_member_roots() {
        let dir = tree(&[
            ("package.json", r#"{"workspaces":["packages/*"]}"#),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("web/package.json", r#"{"name":"web"}"#),
            ("web/bun.lock", ""),
        ]);
        let only_web = JsConfig {
            roots: Roots::Explicit(vec!["./web/package.json".to_string()]),
            ..JsConfig::default()
        };
        assert_eq!(
            summary(&resolve(&only_web, dir.path()).expect("roots")),
            [("web".to_string(), vec![])]
        );

        let member_root = JsConfig {
            roots: Roots::Explicit(vec![
                "package.json".to_string(),
                "packages/a/package.json".to_string(),
            ]),
            ..JsConfig::default()
        };
        let error = resolve(&member_root, dir.path()).expect_err("member root");
        assert_eq!(error.code(), ErrorCode::Conflict);
    }

    #[test]
    fn explicit_roots_reach_into_skipped_test_data_directories() {
        let dir = tree(&[
            ("web/package.json", r#"{"name":"web"}"#),
            ("fixtures/app/package.json", r#"{"name":"app"}"#),
            ("fixtures/app/bun.lock", ""),
        ]);
        assert_eq!(summary(&auto(dir.path())), [("web".to_string(), vec![])]);
        let fixture = JsConfig {
            roots: Roots::Explicit(vec!["fixtures/app/package.json".to_string()]),
            ..JsConfig::default()
        };
        assert_eq!(
            summary(&resolve(&fixture, dir.path()).expect("roots")),
            [("fixtures/app".to_string(), vec![])]
        );
    }

    #[test]
    fn explicit_roots_must_stay_inside_and_exist() {
        let dir = tree(&[("package.json", "{}")]);
        for (root, code) in [
            ("../outside/package.json", ErrorCode::InvalidInput),
            ("web/Cargo.toml", ErrorCode::InvalidInput),
            ("missing/package.json", ErrorCode::NotFound),
        ] {
            let config = JsConfig {
                roots: Roots::Explicit(vec![root.to_string()]),
                ..JsConfig::default()
            };
            let error = resolve(&config, dir.path()).expect_err(root);
            assert_eq!(error.code(), code, "{root}: {error}");
        }
    }

    #[test]
    fn override_selects_the_manager_for_its_root() {
        let dir = tree(&[("web/package.json", "{}"), ("web/package-lock.json", "")]);
        let config = JsConfig {
            package_manager: [("web".to_string(), crate::package_manager::Tool::Bun)].into(),
            ..JsConfig::default()
        };
        let roots = resolve(&config, dir.path()).expect("roots");
        assert_eq!(roots[0].manager, Manager::Bun);
    }

    #[test]
    fn root_keyed_settings_must_name_a_resolved_root() {
        let dir = tree(&[("package.json", "{}"), ("package-lock.json", "")]);
        let pack = |root: &str| JsConfig {
            pack: [(root.to_string(), vec!["bun".to_string()])].into(),
            ..JsConfig::default()
        };
        resolve(&pack("."), dir.path()).expect("the repository root is a root");
        let error = resolve(&pack("web"), dir.path()).expect_err("no web root");
        assert!(
            error.to_string().contains("ecosystems.js.pack.web"),
            "{error}"
        );
        assert!(error.to_string().contains("'web'"), "{error}");

        let manager = |root: &str| JsConfig {
            package_manager: [(root.to_string(), Tool::Npm)].into(),
            ..JsConfig::default()
        };
        resolve(&manager("."), dir.path()).expect("the repository root is a root");
        let error = resolve(&manager("web"), dir.path()).expect_err("no web root");
        assert!(
            error
                .to_string()
                .contains("ecosystems.js.package_manager.web"),
            "{error}"
        );
        assert!(error.to_string().contains("(roots: .)"), "{error}");
    }
}
