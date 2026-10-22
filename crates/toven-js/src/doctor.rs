//! What `toven doctor` checks for JavaScript: the package managers the roots
//! use, whether each root's dependencies are installed and current, and
//! whether the installed package manager matches the root's pin.
//!
//! Every check is offline except the version check, which the engine runs as
//! a command in the root so Corepack and Yarn's `yarnPath` pick the version
//! the root would really use.

use std::path::Path;
use std::time::SystemTime;

use rskit_errors::AppResult;
use rskit_fs::sync_io::{dir, file};
use toven_model::{RepoPath, ToolchainTag};
use toven_ports::{CheckVerdict, DoctorScope, EcosystemCheck};

use crate::config::JsConfig;
use crate::package_manager::{self, Manager, Tool};
use crate::roots::{self, Root};

/// The doctor scope for the JavaScript roots under `project_root`.
///
/// # Errors
/// The roots cannot be resolved (a malformed manifest or an ambiguous
/// package manager), or a root's files cannot be inspected.
pub(crate) fn scope(config: &JsConfig, project_root: &Path) -> AppResult<DoctorScope> {
    let roots = roots::resolve(config, project_root)?;
    let mut toolchains: Vec<ToolchainTag> = Vec::new();
    let mut checks = Vec::new();
    for root in &roots {
        let tool = root.manager.tool().name();
        if !toolchains.iter().any(|tag| tag.tool == tool) {
            toolchains.push(ToolchainTag::new(tool));
        }
        if needs_install(root) {
            checks.push(install_check(project_root, root)?);
        }
        if let Some(pinned) = &root.pinned {
            checks.push(version_check(root, pinned)?);
        }
    }
    let mut scope = DoctorScope::new(toolchains);
    scope.checks = checks;
    Ok(scope)
}

/// Whether the root has anything to install: workspace members to link or a
/// declared dependency. npm writes nothing for a root with neither.
fn needs_install(root: &Root) -> bool {
    let manifest = &root.manifest;
    root.is_workspace()
        || !manifest.dependencies.is_empty()
        || !manifest.dev_dependencies.is_empty()
        || !manifest.optional_dependencies.is_empty()
}

/// The check id for `name` in `root`: bare for the project root, prefixed
/// with the root's directory otherwise.
fn check_id(root: &Root, name: &str) -> String {
    if root.dir.as_os_str().is_empty() {
        name.to_string()
    } else {
        format!("{}/{name}", roots::label(&root.dir))
    }
}

/// The root's directory for a command's directory flag, or `None` for the
/// project root.
fn dir_flag(root: &Root) -> Option<String> {
    (!root.dir.as_os_str().is_empty()).then(|| roots::label(&root.dir))
}

/// Whether the root's dependencies are installed and newer than its lockfile.
fn install_check(project_root: &Path, root: &Root) -> AppResult<EcosystemCheck> {
    let manager = root.manager;
    let dir = project_root.join(&root.dir);
    let id = check_id(root, "install");
    let flag = dir_flag(root);
    let frozen = manager.in_dir(flag.as_deref(), manager.frozen_install());
    let lockfile = package_manager::lockfiles_in(&dir)?
        .into_iter()
        .find(|(tool, _)| *tool == manager.tool())
        .map(|(_, name)| name);
    let Some(lockfile) = lockfile else {
        let install = manager.in_dir(flag.as_deref(), &["install"]);
        return Ok(
            EcosystemCheck::new(id, CheckVerdict::Failed("no lockfile".to_string()))
                .with_fix(format!("{} and commit the lockfile", install.join(" "))),
        );
    };
    let marker = dir.join(manager.install_marker());
    let installed = if manager == Manager::Bun {
        dir::exists(&marker)?
    } else {
        file::exists(&marker)?
    };
    let verdict = if !installed {
        CheckVerdict::Failed("dependencies are not installed".to_string())
    } else if manager != Manager::Bun && modified(&dir.join(lockfile))? > modified(&marker)? {
        CheckVerdict::Failed(format!("dependencies are older than {lockfile}"))
    } else {
        CheckVerdict::Passed
    };
    Ok(EcosystemCheck::new(id, verdict).with_fix(frozen.join(" ")))
}

/// A file's modification time; a file system without one sorts first.
fn modified(path: &Path) -> AppResult<Option<SystemTime>> {
    Ok(file::metadata(path)?.modified)
}

/// The installed package manager must print the root's pinned version.
fn version_check(root: &Root, pinned: &str) -> AppResult<EcosystemCheck> {
    let tool = root.manager.tool().name();
    let fix = if root.manager.tool() == Tool::Bun {
        format!("install bun {pinned}")
    } else {
        format!("run `corepack enable`, or install {tool}@{pinned}")
    };
    Ok(EcosystemCheck::new(
        check_id(root, "package-manager"),
        CheckVerdict::Command {
            argv: vec![tool.to_string(), "--version".to_string()],
            dir: RepoPath::new(roots::label(&root.dir))?,
            version: Some(pinned.to_string()),
        },
    )
    .with_fix(fix))
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use toven_ports::{CheckVerdict, DoctorScope};

    use super::scope;
    use crate::config::JsConfig;

    fn tree(files: &[(&str, &str)]) -> rskit_fs::TempDir {
        let dir = rskit_fs::TempDir::new().expect("tempdir");
        for (file, text) in files {
            dir.write_file(file, text.as_bytes()).expect("write");
        }
        dir
    }

    fn doctor(dir: &rskit_fs::TempDir) -> DoctorScope {
        scope(&JsConfig::default(), dir.path()).expect("scope")
    }

    fn verdicts(scope: &DoctorScope) -> Vec<(String, String)> {
        scope
            .checks
            .iter()
            .map(|check| {
                let verdict = match &check.verdict {
                    CheckVerdict::Passed => "passed".to_string(),
                    CheckVerdict::Failed(reason) => reason.clone(),
                    CheckVerdict::Command { argv, dir, version } => format!(
                        "{} in {} expects {}",
                        argv.join(" "),
                        dir.as_path().display(),
                        version.as_deref().unwrap_or("-")
                    ),
                };
                (check.id.clone(), verdict)
            })
            .collect()
    }

    fn set_modified(dir: &rskit_fs::TempDir, path: &str, at: SystemTime) {
        File::options()
            .write(true)
            .open(dir.path().join(path))
            .expect("open")
            .set_modified(at)
            .expect("set mtime");
    }

    const WORKSPACE: &str = r#"{"private":true,"workspaces":["packages/*"]}"#;
    const MEMBER: &str = r#"{"name":"a"}"#;

    #[test]
    fn each_root_audits_its_own_package_manager_once() {
        let dir = tree(&[
            ("package.json", r#"{"name":"tools"}"#),
            ("package-lock.json", "{}"),
            ("web/package.json", r#"{"name":"web"}"#),
            ("web/bun.lock", "{}"),
            ("api/package.json", r#"{"name":"api"}"#),
            ("api/package-lock.json", "{}"),
        ]);
        let tools: Vec<String> = doctor(&dir)
            .toolchains
            .into_iter()
            .map(|tag| tag.tool)
            .collect();
        assert_eq!(tools, ["npm", "bun"]);
    }

    #[test]
    fn a_missing_or_stale_install_fails_with_the_frozen_install_fix() {
        let dir = tree(&[
            ("package.json", WORKSPACE),
            ("package-lock.json", "{}"),
            ("packages/a/package.json", MEMBER),
        ]);
        let found = doctor(&dir);
        assert_eq!(
            verdicts(&found),
            [(
                "install".to_string(),
                "dependencies are not installed".to_string()
            )]
        );
        assert_eq!(found.checks[0].fix.as_deref(), Some("npm ci"));

        dir.write_file("node_modules/.package-lock.json", b"{}")
            .expect("marker");
        let earlier = SystemTime::now() - Duration::from_mins(1);
        set_modified(&dir, "node_modules/.package-lock.json", earlier);
        assert_eq!(
            verdicts(&doctor(&dir))[0].1,
            "dependencies are older than package-lock.json"
        );

        set_modified(&dir, "package-lock.json", earlier - Duration::from_mins(1));
        assert_eq!(verdicts(&doctor(&dir))[0].1, "passed");
    }

    #[test]
    fn nested_roots_prefix_their_checks_and_flag_their_directory() {
        let dir = tree(&[
            (
                "web/package.json",
                r#"{"name":"web","dependencies":{"x":"1"}}"#,
            ),
            ("web/pnpm-lock.yaml", ""),
        ]);
        let found = doctor(&dir);
        assert_eq!(found.checks[0].id, "web/install");
        assert_eq!(
            found.checks[0].fix.as_deref(),
            Some("pnpm --dir web install --frozen-lockfile")
        );
    }

    #[test]
    fn a_root_without_a_lockfile_is_told_to_create_one() {
        let dir = tree(&[("package.json", r#"{"name":"r","dependencies":{"x":"1"}}"#)]);
        let found = doctor(&dir);
        assert_eq!(verdicts(&found)[0].1, "no lockfile");
        assert_eq!(
            found.checks[0].fix.as_deref(),
            Some("npm install and commit the lockfile")
        );
    }

    #[test]
    fn a_root_with_nothing_to_install_has_no_install_check() {
        let dir = tree(&[("package.json", r#"{"name":"r","scripts":{"x":"y"}}"#)]);
        assert!(doctor(&dir).checks.is_empty());
    }

    #[test]
    fn a_pinned_root_checks_the_installed_version_in_its_directory() {
        let dir = tree(&[
            (
                "web/package.json",
                r#"{"name":"web","packageManager":"pnpm@10.2.0"}"#,
            ),
            ("web/pnpm-lock.yaml", ""),
        ]);
        let found = doctor(&dir);
        assert_eq!(
            verdicts(&found),
            [(
                "web/package-manager".to_string(),
                "pnpm --version in web expects 10.2.0".to_string()
            )]
        );
        assert_eq!(
            found.checks[0].fix.as_deref(),
            Some("run `corepack enable`, or install pnpm@10.2.0")
        );
    }

    #[test]
    fn bun_has_no_install_state_file_so_only_a_missing_install_fails() {
        let dir = tree(&[
            ("package.json", WORKSPACE),
            ("bun.lock", "{}"),
            ("packages/a/package.json", MEMBER),
            ("node_modules/.keep", ""),
        ]);
        set_modified(&dir, "bun.lock", SystemTime::now() + Duration::from_mins(1));
        assert_eq!(verdicts(&doctor(&dir))[0].1, "passed");
    }
}
