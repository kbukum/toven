//! Workspace-level blast radius for JavaScript roots.
//!
//! A change to a root's install inputs can change what every package in the
//! root builds against, so these files invalidate the whole workspace: the
//! root `package.json` (overrides, resolutions, catalogs, root scripts), the
//! package manager's lockfile, `.npmrc`, and each manager's own settings file.
//!
//! JavaScript packages carry no serialization `resource_group`: scripts run in
//! their own package directory and package managers lock their shared stores.

use std::path::Path;

use crate::manifest::{PACKAGE_JSON, PNPM_WORKSPACE};
use crate::package_manager::{Manager, YARNRC_YML};
use crate::roots;

/// The npm-compatible registry settings file every manager reads.
const NPMRC: &str = ".npmrc";

/// Bun's settings file.
const BUNFIG: &str = "bunfig.toml";

/// The repo-relative blast-radius files of the root at `dir`.
pub(crate) fn files(dir: &Path, manager: Manager) -> Vec<String> {
    let mut files = vec![PACKAGE_JSON, NPMRC];
    files.extend(manager.tool().lockfiles());
    match manager {
        Manager::Pnpm => files.push(PNPM_WORKSPACE),
        Manager::YarnBerry => files.push(YARNRC_YML),
        Manager::Bun => files.push(BUNFIG),
        Manager::Npm | Manager::YarnClassic => {}
    }
    let prefix = roots::label(dir);
    files
        .into_iter()
        .map(|file| {
            if prefix == "." {
                file.to_string()
            } else {
                format!("{prefix}/{file}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::files;
    use crate::package_manager::Manager;

    #[test]
    fn root_files_follow_the_manager() {
        assert_eq!(
            files(Path::new(""), Manager::Pnpm),
            [
                "package.json",
                ".npmrc",
                "pnpm-lock.yaml",
                "pnpm-workspace.yaml"
            ]
        );
        assert_eq!(
            files(Path::new("web"), Manager::Bun),
            [
                "web/package.json",
                "web/.npmrc",
                "web/bun.lock",
                "web/bun.lockb",
                "web/bunfig.toml"
            ]
        );
        assert_eq!(
            files(Path::new(""), Manager::YarnClassic),
            ["package.json", ".npmrc", "yarn.lock"]
        );
    }
}
