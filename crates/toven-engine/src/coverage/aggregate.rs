//! Aggregation: attribute emitted profile files to modules, fold each module's
//! metrics, and gate them into a [`CoverageReport`].
//!
//! File attribution keys on the workspace-relative file path: an emitted path
//! is normalized (an absolute path under the project root is made relative) and
//! attributed to the module whose repo-relative `root` is its longest matching
//! prefix. Files that match no module root are ignored. This composes with
//! Toven's affected planning: under `--changed`, `changed` carries the changed
//! files and each module's `changed_line` metric is folded over only those.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use toven_model::{Module, ModuleKey};

use super::gate::gate_module;
use super::metrics::CoverageMetrics;
use super::profile::{CoverageProfile, FileCoverage};
use super::report::CoverageReport;
use super::settings::ResolvedCoverageSettings;

/// The inputs to a coverage aggregation.
pub(super) struct CoverageInputs<'a> {
    /// Project root, used to make absolute emitted paths workspace-relative.
    pub(super) project_root: &'a Path,
    /// The modules the report covers, in report order.
    pub(super) modules: &'a [Module],
    /// The parsed profiles read from the coverage run.
    pub(super) profiles: &'a [CoverageProfile],
    /// Each module's resolved coverage settings, keyed by module key.
    pub(super) settings: &'a BTreeMap<ModuleKey, ResolvedCoverageSettings>,
    /// Changed files (workspace-relative) under `--changed`; `None` otherwise.
    pub(super) changed: Option<&'a BTreeSet<PathBuf>>,
}

/// Attribute profiles to modules, fold metrics, and gate into a report.
///
/// A module with a resolved setting but no attributed files is skipped (nothing
/// was measured for it); a module with no resolved setting is not gated.
#[must_use]
pub(super) fn aggregate(inputs: &CoverageInputs<'_>) -> CoverageReport {
    let attributed = attribute(inputs.project_root, inputs.modules, inputs.profiles);

    let mut modules = Vec::new();
    for module in inputs.modules {
        let key = module.key();
        let Some(files) = attributed.get(&key) else {
            continue;
        };
        let Some(settings) = inputs.settings.get(&key) else {
            continue;
        };
        let file_refs: Vec<&FileCoverage> = files.iter().collect();
        let metrics = CoverageMetrics::compute(&file_refs, inputs.changed);
        modules.push(gate_module(key, metrics, settings));
    }

    CoverageReport {
        modules,
        changed: inputs.changed.is_some(),
    }
}

/// Bucket every profile file under the module whose root is its longest prefix.
fn attribute(
    project_root: &Path,
    modules: &[Module],
    profiles: &[CoverageProfile],
) -> BTreeMap<ModuleKey, Vec<FileCoverage>> {
    let mut buckets: BTreeMap<ModuleKey, Vec<FileCoverage>> = BTreeMap::new();
    for profile in profiles {
        for file in &profile.files {
            let relative = normalize(&file.path, project_root);
            if let Some(module) = longest_match(&relative, modules) {
                let mut attributed = file.clone();
                attributed.path = relative;
                buckets.entry(module.key()).or_default().push(attributed);
            }
        }
    }
    buckets
}

/// Make an absolute path under `project_root` workspace-relative; strip a
/// leading `./`. A path already relative is returned as-is.
fn normalize(path: &Path, project_root: &Path) -> PathBuf {
    let stripped = path
        .strip_prefix(project_root)
        .or_else(|_| path.strip_prefix("./"))
        .unwrap_or(path);
    stripped.to_path_buf()
}

/// The module whose repo-relative root is the longest prefix of `file`.
fn longest_match<'a>(file: &Path, modules: &'a [Module]) -> Option<&'a Module> {
    modules
        .iter()
        .filter(|module| starts_with_root(file, module.root.as_path()))
        .max_by_key(|module| module.root.as_path().components().count())
}

/// Whether `file` is under `root`, treating the repo-root `.` as matching all.
fn starts_with_root(file: &Path, root: &Path) -> bool {
    root == Path::new(".") || file.starts_with(root)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use super::{CoverageInputs, aggregate};
    use crate::coverage::gate::ModuleStatus;
    use crate::coverage::profile::{CoverageProfile, FileCoverage};
    use crate::coverage::settings::ResolvedCoverageSettings;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{CoverageThresholds, Enforcement};

    fn module(name: &str, root: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            RepoPath::new(root).unwrap(),
        )
    }

    fn file(path: &str, hit: u32, found: u32) -> FileCoverage {
        let mut lines = BTreeMap::new();
        for line in 1..=found {
            lines.insert(line, line <= hit);
        }
        FileCoverage {
            path: path.into(),
            lines,
            functions: None,
            regions: None,
        }
    }

    fn settings(line: f64, enforcement: Enforcement) -> ResolvedCoverageSettings {
        ResolvedCoverageSettings {
            thresholds: CoverageThresholds {
                line: Some(line),
                ..CoverageThresholds::default()
            },
            enforcement,
            excluded: false,
        }
    }

    #[test]
    fn attributes_files_to_the_longest_matching_module() {
        let core = module("core", "crates/core");
        let cli = module("cli", "crates/core/cli");
        let profile = CoverageProfile {
            files: vec![
                file("crates/core/src/lib.rs", 9, 10),
                file("crates/core/cli/src/main.rs", 5, 10),
            ],
        };
        let mut resolved = BTreeMap::new();
        resolved.insert(core.key(), settings(80.0, Enforcement::Block));
        resolved.insert(cli.key(), settings(80.0, Enforcement::Block));

        let modules = [core.clone(), cli.clone()];
        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            modules: &modules,
            profiles: &[profile],
            settings: &resolved,
            changed: None,
        });

        // core sees only its own file (90%), cli sees only its file (50% → fails).
        let core_verdict = report
            .modules
            .iter()
            .find(|module| module.module == core.key())
            .expect("core measured");
        assert!((core_verdict.metrics.line - 90.0).abs() < 1e-9);
        assert_eq!(core_verdict.status, ModuleStatus::Passed);
        let cli_verdict = report
            .modules
            .iter()
            .find(|module| module.module == cli.key())
            .expect("cli measured");
        assert_eq!(cli_verdict.status, ModuleStatus::Failed);
        assert!(!report.gate_passed());
    }

    #[test]
    fn normalizes_absolute_emitted_paths() {
        let core = module("core", "crates/core");
        let profile = CoverageProfile {
            files: vec![file("/repo/crates/core/src/lib.rs", 10, 10)],
        };
        let mut resolved = BTreeMap::new();
        resolved.insert(core.key(), settings(90.0, Enforcement::Block));
        let modules = [core];

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            modules: &modules,
            profiles: &[profile],
            settings: &resolved,
            changed: None,
        });
        assert_eq!(report.modules.len(), 1);
        assert_eq!(report.modules[0].status, ModuleStatus::Passed);
    }

    #[test]
    fn cross_module_coverage_from_one_workspace_measurement_is_preserved() {
        // A single workspace measurement attributes coverage by covered-file
        // path, so a downstream module's file is credited even when an upstream
        // module's tests produced that coverage. This is the invariant that a
        // per-module isolated measurement would break: `app` has no tests of its
        // own here, yet its file is fully covered by `core`'s integration tests.
        let core = module("core", "crates/core");
        let app = module("app", "apps/app");
        let workspace_profile = CoverageProfile {
            files: vec![
                file("crates/core/src/lib.rs", 10, 10),
                // Covered by core's tests exercising app, attributed to app by path.
                file("apps/app/src/run.rs", 10, 10),
            ],
        };
        let mut resolved = BTreeMap::new();
        resolved.insert(core.key(), settings(100.0, Enforcement::Block));
        resolved.insert(app.key(), settings(100.0, Enforcement::Block));
        let modules = [core, app.clone()];

        let shared = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            modules: &modules,
            profiles: &[workspace_profile],
            settings: &resolved,
            changed: None,
        });

        // The shared measurement credits app fully, so both modules pass.
        let app_verdict = shared
            .modules
            .iter()
            .find(|module| module.module == app.key())
            .expect("app measured from the shared workspace profile");
        assert!((app_verdict.metrics.line - 100.0).abs() < 1e-9);
        assert_eq!(app_verdict.status, ModuleStatus::Passed);
        assert!(shared.gate_passed());

        // Contrast: an isolated measurement of only app's own tests (none here)
        // never attributes that cross-module coverage — app would be unmeasured
        // (skipped), which is exactly the number the shared measurement rescues.
        let isolated = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            modules: &modules,
            profiles: &[CoverageProfile {
                files: vec![file("crates/core/src/lib.rs", 10, 10)],
            }],
            settings: &resolved,
            changed: None,
        });
        assert!(
            !isolated
                .modules
                .iter()
                .any(|module| module.module == app.key()),
            "isolated core-only profile must not measure app"
        );
    }

    #[test]
    fn changed_scope_gates_changed_line_over_changed_files() {
        let core = module("core", "crates/core");
        let profile = CoverageProfile {
            files: vec![
                file("crates/core/src/lib.rs", 10, 10),
                file("crates/core/src/new.rs", 1, 10),
            ],
        };
        let mut resolved = BTreeMap::new();
        let mut setting = settings(50.0, Enforcement::Block);
        setting.thresholds.changed_line = Some(80.0);
        resolved.insert(core.key(), setting);
        let changed: BTreeSet<PathBuf> =
            std::iter::once(PathBuf::from("crates/core/src/new.rs")).collect();
        let modules = [core];

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            modules: &modules,
            profiles: &[profile],
            settings: &resolved,
            changed: Some(&changed),
        });

        assert!(report.changed);
        let verdict = &report.modules[0];
        // absolute line (55%) clears 50%, but changed-line (10%) fails 80%.
        assert_eq!(verdict.status, ModuleStatus::Failed);
        assert!(
            verdict
                .outcomes
                .iter()
                .any(|outcome| outcome.dimension.as_str() == "changed-line" && !outcome.passed)
        );
    }
}
