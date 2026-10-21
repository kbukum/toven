//! Locate every profile file in the repository before attribution.
//!
//! Tools write file paths three ways: absolute (`cargo llvm-cov`),
//! repo-relative (a Go profile after import-path mapping), or relative to the
//! directory the tool ran in (JS runners run inside each package and write
//! `src/index.ts`). This step turns all of them into lexically normalized
//! repo-relative paths, so attribution only compares prefixes.
//!
//! A relative LCOV path is resolved against the root of the module that staged
//! the profile ([`CoverageProfile::producer`], `<ecosystem>-<module>`). A
//! profile with no matching producer keeps relative paths as repo-relative. Go
//! profiles are already repo-relative and are never rebased. `..` segments are
//! resolved, so a package test that measured a sibling package's file credits
//! that sibling; a path that leaves the repository is dropped.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use toven_model::Module;

use super::profile::{CoverageFormat, CoverageProfile};

/// Make every profile file path repo-relative and normalized.
///
/// # Errors
/// Rejects a producer name that matches more than one module (two federation
/// members with the same ecosystem and module name), because its relative
/// paths could belong to either.
pub(super) fn locate(
    profiles: Vec<CoverageProfile>,
    owners: &[Module],
    project_root: &Path,
) -> AppResult<Vec<CoverageProfile>> {
    let roots = producer_roots(owners);
    profiles
        .into_iter()
        .map(|mut profile| {
            let base = match (&profile.format, profile.producer.as_deref()) {
                (CoverageFormat::Lcov, Some(producer)) => match roots.get(producer) {
                    Some(Producer::One(root)) => Some(*root),
                    Some(Producer::Ambiguous) => return Err(ambiguous(producer)),
                    None => None,
                },
                _ => None,
            };
            profile.files = std::mem::take(&mut profile.files)
                .into_iter()
                .filter_map(|mut file| {
                    file.path = repo_relative(&file.path, project_root, base)?;
                    Some(file)
                })
                .collect();
            Ok(profile)
        })
        .collect()
}

/// What a producer name resolves to.
enum Producer<'a> {
    One(&'a Path),
    Ambiguous,
}

/// Every module's root, keyed by the `<ecosystem>-<module>` name its coverage
/// task stages under.
fn producer_roots(owners: &[Module]) -> BTreeMap<String, Producer<'_>> {
    let mut roots = BTreeMap::new();
    for module in owners {
        let name = format!("{}-{}", module.id.ecosystem, module.id.name);
        roots
            .entry(name)
            .and_modify(|seen| *seen = Producer::Ambiguous)
            .or_insert(Producer::One(module.root.as_path()));
    }
    roots
}

fn ambiguous(producer: &str) -> AppError {
    AppError::invalid_input(
        "coverage profiles",
        format!(
            "coverage profile '{producer}' matches more than one module; \
             stage it under a name that only one module uses"
        ),
    )
}

/// The repo-relative, normalized form of `path`, or `None` when it leaves the
/// repository. An absolute path under `project_root` is made relative; a
/// relative one is joined onto `base` when the producer is known.
fn repo_relative(path: &Path, project_root: &Path, base: Option<&Path>) -> Option<PathBuf> {
    let joined = if let Ok(relative) = path.strip_prefix(project_root) {
        relative.to_path_buf()
    } else if path.is_absolute() {
        return None;
    } else {
        base.map_or_else(|| path.to_path_buf(), |base| base.join(path))
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use rskit_fs::TempDir;
    use rskit_fs::sync_io::dir::create_all;
    use rskit_fs::sync_io::file::write_atomic_replace;
    use toven_model::{MemberId, Module};
    use toven_ports::{CoverageThresholds, Enforcement};

    use super::locate;
    use crate::coverage::aggregate::{CoverageInputs, aggregate};
    use crate::coverage::goimport::GoImportRoots;
    use crate::coverage::metrics::CoverageMetrics;
    use crate::coverage::profile::{CoverageFormat, CoverageProfile, FileCoverage};
    use crate::coverage::read::read_profiles;
    use crate::coverage::report::CoverageReport;
    use crate::coverage::settings::ResolvedCoverageSettings;
    use crate::coverage::test_support::{go_module, module};

    fn file(path: &str) -> FileCoverage {
        FileCoverage {
            path: path.into(),
            lines: BTreeMap::from([(1, true)]),
            functions: None,
            regions: None,
        }
    }

    fn profile(format: CoverageFormat, producer: Option<&str>, paths: &[&str]) -> CoverageProfile {
        CoverageProfile {
            format,
            producer: producer.map(str::to_string),
            files: paths.iter().map(|path| file(path)).collect(),
        }
    }

    fn paths(profiles: &[CoverageProfile]) -> Vec<PathBuf> {
        profiles
            .iter()
            .flat_map(|profile| profile.files.iter().map(|file| file.path.clone()))
            .collect()
    }

    fn js_modules() -> Vec<Module> {
        vec![
            module("js", "web", ".", Some("package.json")),
            module(
                "js",
                "packages-app",
                "packages/app",
                Some("packages/app/package.json"),
            ),
            module(
                "js",
                "packages-core",
                "packages/core",
                Some("packages/core/package.json"),
            ),
            module(
                "js",
                "packages-app-plugin",
                "packages/app/plugin",
                Some("packages/app/plugin/package.json"),
            ),
        ]
    }

    #[test]
    fn resolves_producer_relative_lcov_paths_against_the_module_root() {
        let located = locate(
            vec![profile(
                CoverageFormat::Lcov,
                Some("js-packages-app"),
                &[
                    "src/index.ts",
                    "./src/util.ts",
                    "../core/src/lib.ts",
                    "/repo/packages/app/src/abs.ts",
                ],
            )],
            &js_modules(),
            Path::new("/repo"),
        )
        .expect("locates");
        assert_eq!(
            paths(&located),
            [
                "packages/app/src/index.ts",
                "packages/app/src/util.ts",
                "packages/core/src/lib.ts",
                "packages/app/src/abs.ts",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn keeps_repo_relative_paths_without_a_known_producer_or_for_go() {
        let modules = js_modules();
        let located = locate(
            vec![
                profile(
                    CoverageFormat::Lcov,
                    Some("rust"),
                    &["crates/core/src/lib.rs"],
                ),
                profile(CoverageFormat::Lcov, None, &["./crates/cli/src/main.rs"]),
                profile(
                    CoverageFormat::GoProfile,
                    Some("js-packages-app"),
                    &["services/api/handler.go"],
                ),
            ],
            &modules,
            Path::new("/repo"),
        )
        .expect("locates");
        assert_eq!(
            paths(&located),
            [
                "crates/core/src/lib.rs",
                "crates/cli/src/main.rs",
                "services/api/handler.go",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn drops_paths_that_leave_the_repository() {
        let located = locate(
            vec![profile(
                CoverageFormat::Lcov,
                Some("js-packages-app"),
                &["../../../outside.ts", "/elsewhere/lib.ts", "src/kept.ts"],
            )],
            &js_modules(),
            Path::new("/repo"),
        )
        .expect("locates");
        assert_eq!(paths(&located), [PathBuf::from("packages/app/src/kept.ts")]);
    }

    #[test]
    fn rejects_a_producer_name_shared_by_two_modules() {
        let mut first = module("js", "core", "a", None);
        first.member = Some(MemberId::new("one").expect("member"));
        let mut second = module("js", "core", "b", None);
        second.member = Some(MemberId::new("two").expect("member"));
        let error = locate(
            vec![profile(
                CoverageFormat::Lcov,
                Some("js-core"),
                &["src/a.ts"],
            )],
            &[first, second],
            Path::new("/repo"),
        )
        .expect_err("ambiguous producer rejected");
        assert!(
            error.to_string().contains("more than one module"),
            "{error}"
        );
    }

    fn settings() -> ResolvedCoverageSettings {
        ResolvedCoverageSettings {
            thresholds: CoverageThresholds {
                line: Some(50.0),
                ..CoverageThresholds::default()
            },
            enforcement: Enforcement::Block,
            excluded: false,
        }
    }

    /// Stage profiles under a temp coverage dir, then read, locate, and
    /// aggregate them over `modules` like the coverage verb does.
    fn measure(
        staged: &[(&str, &str)],
        modules: &[Module],
        go_roots: &GoImportRoots,
        changed: Option<&BTreeSet<PathBuf>>,
    ) -> CoverageReport {
        let temp = TempDir::new().expect("temp dir");
        let dir = temp.path().join("coverage");
        for (target, fixture) in staged {
            let path = dir.join(target);
            create_all(path.parent().expect("parent")).expect("mkdir");
            let contents = toven_testkit::coverage_profile_string(fixture).expect("fixture");
            write_atomic_replace(&path, contents.as_bytes(), "locate-test").expect("stage");
        }
        let profiles = read_profiles(&dir, go_roots).expect("reads");
        let located = locate(profiles, modules, temp.path()).expect("locates");
        let resolved = modules
            .iter()
            .map(|module| (module.key(), settings()))
            .collect();
        aggregate(&CoverageInputs {
            owners: modules,
            modules,
            profiles: &located,
            settings: &resolved,
            changed,
        })
    }

    fn metrics(report: &CoverageReport, module: &Module) -> Option<CoverageMetrics> {
        report
            .modules
            .iter()
            .find(|verdict| verdict.module == module.key())
            .map(|verdict| verdict.metrics)
    }

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1e-6
    }

    /// Whether a measured percentage matches an expected ratio.
    fn same(actual: Option<f64>, expected: Option<f64>) -> bool {
        match (actual, expected) {
            (Some(actual), Some(expected)) => close(actual, expected * 100.0),
            (None, None) => true,
            _ => false,
        }
    }

    #[test]
    fn real_runner_profiles_credit_the_package_and_never_the_root() {
        // Captured from vitest 3 (v8), jest 30, c8 10 over `node --test`, and
        // bun 1.3 against the same `src/math.js`; each writes `SF:src/math.js`
        // relative to the package it ran in.
        let root = module("js", "web", ".", Some("package.json"));
        let lib = module("js", "packages-lib", "packages/lib", None);
        let modules = [root.clone(), lib.clone()];
        let cases = [
            ("vitest", 11.0 / 13.0, Some(1.0), Some(3.0 / 4.0)),
            ("jest", 4.0 / 6.0, Some(2.0 / 3.0), Some(1.0 / 2.0)),
            ("c8", 11.0 / 16.0, Some(2.0 / 3.0), Some(3.0 / 4.0)),
            ("bun", 8.0 / 10.0, Some(3.0 / 4.0), None),
        ];
        for (runner, line, function, region) in cases {
            let target = "js-packages-lib/lcov.info";
            let report = measure(
                &[(target, &format!("js/{runner}.lcov"))],
                &modules,
                &GoImportRoots::default(),
                None,
            );
            assert!(metrics(&report, &root).is_none(), "{runner}: root credited");
            let measured = metrics(&report, &lib).expect("package measured");
            assert!(
                close(measured.line, line * 100.0),
                "{runner}: line {measured:?}"
            );
            assert!(
                same(measured.function, function),
                "{runner}: function {measured:?}"
            );
            assert!(
                same(measured.region, region),
                "{runner}: region {measured:?}"
            );
        }
    }

    #[test]
    fn a_nested_package_file_is_never_credited_to_its_parent() {
        let modules = js_modules();
        let located = locate(
            vec![profile(
                CoverageFormat::Lcov,
                Some("js-packages-app"),
                &["src/index.ts", "plugin/src/index.ts"],
            )],
            &modules,
            Path::new("/repo"),
        )
        .expect("locates");
        let resolved = modules
            .iter()
            .map(|module| (module.key(), settings()))
            .collect();
        let report = aggregate(&CoverageInputs {
            owners: &modules,
            modules: &modules,
            profiles: &located,
            settings: &resolved,
            changed: None,
        });
        let measured: Vec<String> = report
            .modules
            .iter()
            .map(|verdict| verdict.module.to_string())
            .collect();
        assert_eq!(measured, ["js:packages-app", "js:packages-app-plugin"]);
    }

    #[test]
    fn changed_lines_apply_to_package_relative_profiles() {
        let lib = module("js", "packages-lib", "packages/lib", None);
        let changed = BTreeSet::from([PathBuf::from("packages/lib/src/math.js")]);
        let report = measure(
            &[("js-packages-lib/lcov.info", "js/vitest.lcov")],
            std::slice::from_ref(&lib),
            &GoImportRoots::default(),
            Some(&changed),
        );
        let measured = metrics(&report, &lib).expect("measured");
        assert!(
            measured
                .changed_line
                .is_some_and(|value| close(value, 11.0 / 13.0 * 100.0)),
            "{measured:?}"
        );
    }

    #[test]
    fn mixed_go_rust_and_js_profiles_aggregate_together() {
        let core = module("rust", "toven-core", "crates/toven-core", None);
        let cli = module("rust", "toven-cli", "crates/toven-cli", None);
        let api = go_module("api", "services/api", "example.com/repo/services/api");
        let worker = go_module(
            "worker",
            "services/worker",
            "example.com/repo/services/worker",
        );
        let lib = module("js", "packages-lib", "packages/lib", None);
        let modules = [core, cli, api, worker, lib.clone()];
        let report = measure(
            &[
                ("rust-workspace.lcov", "rust.lcov"),
                ("go-api.out", "go.out"),
                ("js-packages-lib/lcov.info", "js/c8.lcov"),
            ],
            &modules,
            &GoImportRoots::from_modules(&modules),
            None,
        );
        let measured: BTreeSet<String> = report
            .modules
            .iter()
            .map(|verdict| verdict.module.to_string())
            .collect();
        assert_eq!(
            measured,
            BTreeSet::from(
                [
                    "go:api",
                    "go:worker",
                    "js:packages-lib",
                    "rust:toven-cli",
                    "rust:toven-core"
                ]
                .map(str::to_string)
            )
        );
        let js = metrics(&report, &lib).expect("js measured");
        assert!(close(js.line, 11.0 / 16.0 * 100.0), "{js:?}");
    }
}
