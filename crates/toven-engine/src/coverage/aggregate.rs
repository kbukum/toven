//! Aggregation: attribute emitted profile files to modules, fold each module's
//! metrics, and gate them into a [`CoverageReport`].
//!
//! File attribution keys on the workspace-relative file path: an emitted path
//! is normalized (an absolute path under the project root is made relative) and
//! attributed to the discovered module whose repo-relative `root` is its
//! longest matching prefix. Only modules that can produce the profile's format
//! are candidates: a Go profile goes to `go.mod` modules, an LCOV profile to
//! the others. When two candidates share that root, a selected module wins over
//! an unselected one, then the lower module key. Only selected modules are
//! reported; files that match no candidate, or belong to an unselected module,
//! are ignored. This composes with Toven's affected planning: under
//! `--changed`, `changed` carries the changed files and each module's
//! `changed_line` metric is folded over only those.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use toven_model::{Module, ModuleKey};

use super::gate::gate_module;
use super::goimport::is_go_module;
use super::metrics::CoverageMetrics;
use super::profile::{CoverageFormat, CoverageProfile, FileCoverage};
use super::report::CoverageReport;
use super::settings::ResolvedCoverageSettings;

/// The inputs to a coverage aggregation.
pub(super) struct CoverageInputs<'a> {
    /// Project root, used to make absolute emitted paths workspace-relative.
    pub(super) project_root: &'a Path,
    /// Every discovered module. Files are attributed against all of them, so a
    /// file owned by an unselected module is never credited to a selected one.
    pub(super) owners: &'a [Module],
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
    let selected: BTreeSet<ModuleKey> = inputs.modules.iter().map(Module::key).collect();
    let attributed = attribute(
        inputs.project_root,
        inputs.owners,
        &selected,
        inputs.profiles,
    );

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

/// Bucket every profile file under the candidate module whose root is its
/// longest prefix.
fn attribute(
    project_root: &Path,
    modules: &[Module],
    selected: &BTreeSet<ModuleKey>,
    profiles: &[CoverageProfile],
) -> BTreeMap<ModuleKey, Vec<FileCoverage>> {
    let mut buckets: BTreeMap<ModuleKey, Vec<FileCoverage>> = BTreeMap::new();
    for profile in profiles {
        let candidates: Vec<&Module> = modules
            .iter()
            .filter(|module| produces(profile.format, module))
            .collect();
        for file in &profile.files {
            let relative = normalize(&file.path, project_root);
            if let Some(module) = longest_match(&relative, &candidates, selected) {
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

/// Whether `module` can have produced a profile in `format`: Go profiles come
/// only from Go modules, LCOV only from the others.
fn produces(format: CoverageFormat, module: &Module) -> bool {
    match format {
        CoverageFormat::GoProfile => is_go_module(module),
        CoverageFormat::Lcov => !is_go_module(module),
    }
}

/// The candidate whose repo-relative root is the longest prefix of `file`.
///
/// A tie on root depth (two ecosystems sharing a directory) prefers a selected
/// module, then the lower module key, so the result never depends on discovery
/// order.
fn longest_match<'a>(
    file: &Path,
    modules: &[&'a Module],
    selected: &BTreeSet<ModuleKey>,
) -> Option<&'a Module> {
    modules
        .iter()
        .copied()
        .filter(|module| starts_with_root(file, module.root.as_path()))
        .max_by(|left, right| {
            depth(left)
                .cmp(&depth(right))
                .then_with(|| {
                    selected
                        .contains(&left.key())
                        .cmp(&selected.contains(&right.key()))
                })
                .then_with(|| right.key().cmp(&left.key()))
        })
}

/// The number of real segments in `module`'s root; the repo root `.` is 0, so
/// it never ties with a one-level nested root.
fn depth(module: &Module) -> usize {
    module
        .root
        .as_path()
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .count()
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
    use crate::coverage::profile::{CoverageFormat, CoverageProfile, FileCoverage};
    use crate::coverage::settings::ResolvedCoverageSettings;
    use crate::coverage::test_support::go_module;
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
            format: CoverageFormat::Lcov,
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
            owners: &modules,
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
    fn go_import_paths_attribute_to_nested_modules() {
        // runlab's shape: a root module plus two nested modules. The profile
        // names files by import path; each module must get only its own files.
        let root = go_module("runlab", ".", "example.com/runlab");
        let services = go_module(
            "go-services",
            "go-services",
            "example.com/runlab/go-services",
        );
        let generated = go_module("gen-go", "gen/go", "example.com/runlab/gen/go");
        let modules = [root.clone(), services.clone(), generated.clone()];
        let contents = toven_testkit::coverage_profile_string("go-nested.out").expect("fixture");
        let profile = crate::coverage::goprofile::parse(
            &contents,
            &crate::coverage::goimport::GoImportRoots::from_modules(&modules),
        )
        .expect("parses");
        let mut resolved = BTreeMap::new();
        for module in &modules {
            resolved.insert(module.key(), settings(50.0, Enforcement::Block));
        }

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            owners: &modules,
            modules: &modules,
            profiles: &[profile],
            settings: &resolved,
            changed: None,
        });

        let line = |key| {
            report
                .modules
                .iter()
                .find(|module| module.module == key)
                .map(|module| module.metrics.line)
        };
        // go-services: lines 10-14 hit, 16-18 missed → 5/8.
        assert_eq!(line(services.key()), Some(62.5));
        assert_eq!(line(generated.key()), Some(100.0));
        assert_eq!(line(root.key()), Some(100.0));
        assert!(report.gate_passed());
    }

    #[test]
    fn go_changed_files_match_mapped_paths() {
        // The changed-line floor compares repo paths, so it only works once
        // import paths are mapped.
        let services = go_module(
            "go-services",
            "go-services",
            "example.com/runlab/go-services",
        );
        let modules = [services.clone()];
        let contents = toven_testkit::coverage_profile_string("go-nested.out").expect("fixture");
        let profile = crate::coverage::goprofile::parse(
            &contents,
            &crate::coverage::goimport::GoImportRoots::from_modules(&modules),
        )
        .expect("parses");
        let mut setting = settings(0.0, Enforcement::Block);
        setting.thresholds.changed_line = Some(80.0);
        let resolved: BTreeMap<_, _> = std::iter::once((services.key(), setting)).collect();
        let changed: BTreeSet<PathBuf> =
            std::iter::once(PathBuf::from("go-services/cmd/gateway/main.go")).collect();

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            owners: &modules,
            modules: &modules,
            profiles: &[profile],
            settings: &resolved,
            changed: Some(&changed),
        });

        assert_eq!(report.modules.len(), 1);
        assert_eq!(report.modules[0].status, ModuleStatus::Failed);
    }

    #[test]
    fn normalizes_absolute_emitted_paths() {
        let core = module("core", "crates/core");
        let profile = CoverageProfile {
            format: CoverageFormat::Lcov,
            files: vec![file("/repo/crates/core/src/lib.rs", 10, 10)],
        };
        let mut resolved = BTreeMap::new();
        resolved.insert(core.key(), settings(90.0, Enforcement::Block));
        let modules = [core];

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            owners: &modules,
            modules: &modules,
            profiles: &[profile],
            settings: &resolved,
            changed: None,
        });
        assert_eq!(report.modules.len(), 1);
        assert_eq!(report.modules[0].status, ModuleStatus::Passed);
    }

    #[test]
    fn an_out_of_scope_nested_module_is_not_credited_to_its_active_parent() {
        // A stale or whole-workspace profile can carry a nested module's files.
        // When only the parent is selected, those files still belong to the
        // nested module and must not change the parent's number.
        let root = module("root", ".");
        let nested = module("nested", "nested");
        let profile = CoverageProfile {
            format: CoverageFormat::Lcov,
            files: vec![file("src/lib.rs", 10, 10), file("nested/src/lib.rs", 0, 10)],
        };
        let mut resolved = BTreeMap::new();
        resolved.insert(root.key(), settings(100.0, Enforcement::Block));
        let owners = [root.clone(), nested];
        let scope = [root];

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            owners: &owners,
            modules: &scope,
            profiles: &[profile],
            settings: &resolved,
            changed: None,
        });

        assert_eq!(report.modules.len(), 1);
        assert!((report.modules[0].metrics.line - 100.0).abs() < 1e-9);
        assert_eq!(report.modules[0].status, ModuleStatus::Passed);
    }

    #[test]
    fn each_profile_format_credits_only_its_own_ecosystem_at_a_shared_root() {
        // A command module and a Go module both rooted at `.`: Go statement
        // coverage must reach the Go module and LCOV the command module, no
        // matter which one discovery lists last.
        let command = crate::coverage::test_support::module("command", "app", ".", None);
        let go = go_module("app", ".", "example.com/app");
        let modules = [go.clone(), command.clone()];
        let mut resolved = BTreeMap::new();
        resolved.insert(go.key(), settings(0.0, Enforcement::Block));
        resolved.insert(command.key(), settings(0.0, Enforcement::Block));

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            owners: &modules,
            modules: &modules,
            profiles: &[
                CoverageProfile {
                    format: CoverageFormat::GoProfile,
                    files: vec![file("cmd/main.go", 5, 10)],
                },
                CoverageProfile {
                    format: CoverageFormat::Lcov,
                    files: vec![file("src/lib.rs", 10, 10)],
                },
            ],
            settings: &resolved,
            changed: None,
        });

        let line = |key| {
            report
                .modules
                .iter()
                .find(|module| module.module == key)
                .map(|module| module.metrics.line)
        };
        assert_eq!(line(go.key()), Some(50.0));
        assert_eq!(line(command.key()), Some(100.0));
    }

    #[test]
    fn a_shared_root_tie_prefers_the_selected_module() {
        // An unselected module listed last at the same root must not take the
        // selected module's files.
        let selected =
            crate::coverage::test_support::module("rust", "core", ".", Some("Cargo.toml"));
        let unselected = crate::coverage::test_support::module("command", "tools", ".", None);
        let owners = [selected.clone(), unselected];
        let scope = [selected.clone()];
        let mut resolved = BTreeMap::new();
        resolved.insert(selected.key(), settings(100.0, Enforcement::Block));

        let report = aggregate(&CoverageInputs {
            project_root: std::path::Path::new("/repo"),
            owners: &owners,
            modules: &scope,
            profiles: &[CoverageProfile {
                format: CoverageFormat::Lcov,
                files: vec![file("src/lib.rs", 10, 10)],
            }],
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
            format: CoverageFormat::Lcov,
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
            owners: &modules,
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
            owners: &modules,
            modules: &modules,
            profiles: &[CoverageProfile {
                format: CoverageFormat::Lcov,
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
            format: CoverageFormat::Lcov,
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
            owners: &modules,
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
