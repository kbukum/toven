//! Config-less detection: find the JavaScript roots and carry their wizard
//! facts.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;
use toven_ports::Detection;

use crate::config::JsConfig;
use crate::discovery::{self, JS, root_packages};
use crate::manifest::PackageJson;
use crate::package_manager::Manager;
use crate::roots;
use crate::test_runner::TestRunner;

/// The scripts a package manager runs itself around install, pack, publish,
/// and version. They are hooks, not gates.
const LIFECYCLE_SCRIPTS: [&str; 15] = [
    "dependencies",
    "install",
    "pack",
    "postinstall",
    "postpack",
    "postpublish",
    "postversion",
    "preinstall",
    "prepack",
    "prepare",
    "prepublish",
    "prepublishOnly",
    "preversion",
    "publish",
    "version",
];

/// The script whose output other packages consume.
pub(crate) const BUILD_SCRIPT: &str = "build";

/// The script the `coverage` task runs with LCOV flags.
pub(crate) const TEST_SCRIPT: &str = "test";

/// The adapter-owned facts a JavaScript [`Detection`] carries to
/// [`render`](crate::render).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JsFacts {
    /// The package manager every root uses, by [`Manager::id`].
    pub(crate) manager: String,
    /// The scripts any package declares, without lifecycle hooks.
    pub(crate) scripts: BTreeSet<String>,
    /// Whether a package with a `build` script publishes files from `dist`.
    pub(crate) dist: bool,
    /// The runner every package's `test` script calls, when they all call the
    /// same recognized runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) test_runner: Option<TestRunner>,
    /// Every root's install inputs that exist (root manifest, lockfile,
    /// registry and manager settings), repo-relative. They change what each
    /// package builds against, so cacheable tasks hash them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) install_inputs: Vec<String>,
}

impl JsFacts {
    /// Decode the facts from a [`Detection`]'s opaque table.
    ///
    /// # Errors
    /// Returns an error if the facts table is not the shape this adapter wrote.
    pub(crate) fn from_detection(detection: &Detection) -> AppResult<Self> {
        detection.facts.clone().try_into().map_err(|error| {
            AppError::new(ErrorCode::Internal, "invalid js detection facts").with_cause(error)
        })
    }

    /// The package manager the facts record.
    ///
    /// # Errors
    /// Returns an error if the recorded id is not a known manager.
    pub(crate) fn manager(&self) -> AppResult<Manager> {
        Manager::from_id(&self.manager).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "invalid js detection facts: unknown manager '{}'",
                    self.manager
                ),
            )
        })
    }

    /// Encode the facts into an opaque [`Table`] for a [`Detection`].
    fn to_table(&self) -> AppResult<Table> {
        Table::try_from(self).map_err(|error| {
            AppError::new(ErrorCode::Internal, "encode js facts").with_cause(error)
        })
    }
}

/// Detect the JavaScript roots under `project_root` and, if any, return a
/// [`Detection`] carrying their package manager and scripts. Returns `None`
/// when no `package.json` is found.
///
/// # Errors
/// A malformed manifest, an ambiguous package manager, or roots that use
/// different package managers. Onboarding writes one task table for the
/// ecosystem, so for now every root must share one package manager.
pub(crate) fn detect(project_root: &Path) -> AppResult<Option<Detection>> {
    let roots = roots::resolve(&JsConfig::default(), project_root)?;
    let Some(first) = roots.first() else {
        return Ok(None);
    };
    if let Some(other) = roots.iter().find(|root| root.manager != first.manager) {
        return Err(AppError::new(
            ErrorCode::Conflict,
            format!(
                "JavaScript root '{}' uses {} but root '{}' uses {}; toven init writes one \
                 [ecosystems.js] task table, so use one package manager for every root or \
                 write the tasks by hand",
                roots::label(&first.dir),
                first.manager.id(),
                roots::label(&other.dir),
                other.manager.id()
            ),
        ));
    }

    let mut scripts = BTreeSet::new();
    let mut dist = false;
    let mut runners = BTreeSet::new();
    let mut install_inputs = BTreeSet::new();
    for root in &roots {
        install_inputs.extend(
            discovery::install_inputs(&root.dir, root.manager)
                .into_iter()
                .filter(|file| project_root.join(file).is_file()),
        );
        for (_, manifest) in root_packages(project_root, root)? {
            scripts.extend(gate_scripts(&manifest));
            dist |= manifest.scripts.contains_key(BUILD_SCRIPT) && manifest.references_dist();
            if let Some(test) = manifest.scripts.get(TEST_SCRIPT) {
                runners.insert(TestRunner::from_script(test));
            }
        }
    }
    // For now one `coverage` task serves every package, so it needs one runner.
    let test_runner = match runners.into_iter().collect::<Vec<_>>().as_slice() {
        [Some(runner)] => Some(*runner),
        _ => None,
    };
    let facts = JsFacts {
        manager: first.manager.id().to_string(),
        scripts,
        dist,
        test_runner,
        install_inputs: install_inputs.into_iter().collect(),
    };
    Ok(Some(Detection::new(
        EcosystemId::new(JS)?,
        facts.to_table()?,
    )))
}

/// The package's scripts that can be gates: not a lifecycle script and not a
/// `pre`/`post` hook of another script.
fn gate_scripts(manifest: &PackageJson) -> impl Iterator<Item = String> + '_ {
    manifest.scripts.keys().filter_map(|script| {
        let hook = ["pre", "post"].iter().any(|prefix| {
            script
                .strip_prefix(prefix)
                .is_some_and(|target| manifest.scripts.contains_key(target))
        });
        (!hook && !LIFECYCLE_SCRIPTS.contains(&script.as_str())).then(|| script.clone())
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use rskit_errors::ErrorCode;
    use rskit_fs::TempDir;

    use super::{JsFacts, detect};
    use crate::test_runner::TestRunner;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn no_package_json_yields_none() {
        let dir = TempDir::new().unwrap();
        assert!(detect(dir.path()).unwrap().is_none());
    }

    #[test]
    fn records_the_manager_gate_scripts_and_dist() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"private":true,"workspaces":["packages/*"],
               "scripts":{"lint":"eslint .","test":"npm test --workspaces","prepare":"husky"}}"#,
        );
        write(dir.path(), "package-lock.json", "{}");
        write(
            dir.path(),
            "packages/a/package.json",
            r#"{"name":"a","main":"dist/index.js",
               "scripts":{"build":"tsc","pretest":"tsc","test":"node --test","test:e2e":"x"}}"#,
        );

        let detection = detect(dir.path()).unwrap().expect("detection");
        assert_eq!(detection.ecosystem.as_str(), "js");
        let facts = JsFacts::from_detection(&detection).unwrap();
        assert_eq!(facts.manager, "npm");
        assert_eq!(
            facts.scripts.iter().map(String::as_str).collect::<Vec<_>>(),
            ["build", "lint", "test", "test:e2e"]
        );
        assert!(facts.dist);
        // Only the install inputs that exist are recorded.
        assert_eq!(facts.install_inputs, ["package-lock.json", "package.json"]);
        // `node --test` is not a runner the coverage task can drive.
        assert_eq!(facts.test_runner, None);
    }

    #[test]
    fn records_a_test_runner_every_package_shares() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"private":true,"workspaces":["packages/*"],"scripts":{"lint":"eslint ."}}"#,
        );
        write(dir.path(), "package-lock.json", "{}");
        write(
            dir.path(),
            "packages/a/package.json",
            r#"{"name":"a","scripts":{"test":"vitest run"}}"#,
        );
        write(
            dir.path(),
            "packages/b/package.json",
            r#"{"name":"b","scripts":{"test":"vitest"}}"#,
        );
        let facts = JsFacts::from_detection(&detect(dir.path()).unwrap().unwrap()).unwrap();
        assert_eq!(facts.test_runner, Some(TestRunner::Vitest));

        write(
            dir.path(),
            "packages/b/package.json",
            r#"{"name":"b","scripts":{"test":"jest"}}"#,
        );
        let facts = JsFacts::from_detection(&detect(dir.path()).unwrap().unwrap()).unwrap();
        assert_eq!(
            facts.test_runner, None,
            "mixed runners get no coverage task"
        );
    }

    #[test]
    fn dist_needs_a_build_script() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"a","main":"dist/index.js","scripts":{"test":"x"}}"#,
        );
        let facts = JsFacts::from_detection(&detect(dir.path()).unwrap().unwrap()).unwrap();
        assert!(!facts.dist);
    }

    #[test]
    fn roots_with_different_managers_conflict() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "package.json", r#"{"name":"a"}"#);
        write(dir.path(), "package-lock.json", "{}");
        write(dir.path(), "web/package.json", r#"{"name":"web"}"#);
        write(dir.path(), "web/bun.lock", "{}");
        let error = detect(dir.path()).expect_err("conflict");
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(error.to_string().contains("one package manager"), "{error}");
    }
}
