use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::case::ResolvedInvocation;

pub fn assert_cargo_waves_match_output(repo: &Path, stdout: &str) {
    let actual = parse_waves(stdout);
    let expected = cargo_metadata_waves(repo);

    assert_eq!(
        expected, actual,
        "planner waves should match cargo metadata dependencies"
    );
}

pub fn expected_affected_modules(repo: &Path, invocation: &ResolvedInvocation) -> Vec<String> {
    let base = invocation.base.as_deref().unwrap_or("HEAD");
    let diff_base = if invocation.merge_base.unwrap_or(false) {
        git_capture(repo, &["merge-base", base, "HEAD"])
            .trim()
            .to_owned()
    } else {
        base.to_owned()
    };

    let diff = git_capture(repo, &["diff", "--name-only", &diff_base]);
    let changed_paths = diff
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(repo.join("Cargo.toml"))
        .exec()
        .expect("load cargo metadata");

    let global_change = changed_paths
        .iter()
        .any(|path| matches!(*path, "Cargo.toml" | "Cargo.lock" | "toven.toml"));
    let packages = metadata.workspace_packages();
    if global_change {
        return packages
            .into_iter()
            .map(|package| affected_module_id(&package.name))
            .collect();
    }

    let mut dependencies = std::collections::BTreeMap::new();
    let mut affected = BTreeSet::new();
    for package in &packages {
        dependencies.insert(
            String::from(package.name.as_str()),
            package
                .dependencies
                .iter()
                .filter_map(|dependency| {
                    dependency
                        .path
                        .as_ref()
                        .map(|_| String::from(dependency.name.as_str()))
                })
                .collect::<BTreeSet<_>>(),
        );

        let package_root = package
            .manifest_path
            .parent()
            .expect("package manifest has parent")
            .strip_prefix(repo)
            .expect("package is under repo")
            .to_string();
        let package_root = package_root.trim_start_matches("./");
        let package_root = if package_root.is_empty() {
            ".".to_owned()
        } else {
            package_root.to_owned()
        };

        let changed = changed_paths.iter().any(|changed_path| {
            package_root == "." || changed_path.starts_with(&format!("{package_root}/"))
        });

        if changed {
            affected.insert(String::from(package.name.as_str()));
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (package, deps) in &dependencies {
            if !affected.contains(package)
                && deps.iter().any(|dependency| affected.contains(dependency))
            {
                affected.insert(package.clone());
                changed = true;
            }
        }
    }

    packages
        .into_iter()
        .filter_map(|package| {
            if affected.contains(package.name.as_str()) {
                Some(affected_module_id(&package.name))
            } else {
                None
            }
        })
        .collect()
}

pub fn assert_affected_output_matches(repo: &Path, stdout: &str, expected: &[String]) {
    let actual = parse_modules(stdout);
    let expected = expected.iter().cloned().collect::<BTreeSet<_>>();

    assert_eq!(
        expected,
        actual,
        "affected modules should match git diff for {}",
        repo.display()
    );
}

fn parse_waves(stdout: &str) -> Vec<Vec<String>> {
    let mut waves = Vec::new();
    let mut current = Vec::new();

    for line in stdout.lines() {
        if line.starts_with("unit: ") {
            if !current.is_empty() {
                current.sort();
                waves.push(current);
                current = Vec::new();
            }
            continue;
        }

        if let Some(modules) = line.strip_prefix("modules: ") {
            current.extend(parse_module_list(modules));
        }
    }

    if !current.is_empty() {
        current.sort();
        waves.push(current);
    }

    waves
}

fn parse_modules(stdout: &str) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let mut in_modules = false;
    for line in stdout.lines() {
        if line == "modules:" {
            in_modules = true;
            continue;
        }
        if line.is_empty() {
            in_modules = false;
        }

        if in_modules && let Some(module) = line.strip_prefix("- ") {
            modules.insert(
                module
                    .split_whitespace()
                    .next()
                    .unwrap_or(module)
                    .to_owned(),
            );
        } else if let Some(module_list) = line.strip_prefix("modules: ") {
            modules.extend(parse_module_list(module_list));
        }
    }
    modules
}

fn parse_module_list(modules: &str) -> Vec<String> {
    if modules == "none" {
        return Vec::new();
    }

    modules
        .split(',')
        .map(str::trim)
        .filter(|module| !module.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn cargo_metadata_waves(repo: &Path) -> Vec<Vec<String>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(repo.join("Cargo.toml"))
        .exec()
        .expect("load cargo metadata");

    let workspace_members = metadata
        .workspace_members
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut remaining = metadata
        .packages
        .iter()
        .filter(|package| workspace_members.contains(&package.id))
        .map(|package| (package.id.clone(), package))
        .collect::<Vec<_>>();
    let mut completed = BTreeSet::new();
    let mut waves = Vec::new();

    while !remaining.is_empty() {
        let mut wave = Vec::new();
        let mut wave_completed = Vec::new();
        let mut next_remaining = Vec::new();

        for (package_id, package) in remaining {
            let workspace_dependencies = package
                .dependencies
                .iter()
                .filter_map(|dependency| dependency.path.as_ref().map(|_| dependency.name.as_str()))
                .collect::<BTreeSet<_>>();

            if workspace_dependencies
                .iter()
                .all(|dependency| completed.contains(*dependency))
            {
                wave.push(module_id(&package.name));
                wave_completed.push(package.name.to_string());
            } else {
                next_remaining.push((package_id, package));
            }
        }

        assert!(
            !wave.is_empty(),
            "cargo metadata dependency graph made no progress"
        );
        completed.extend(wave_completed);
        wave.sort();
        waves.push(wave);
        remaining = next_remaining;
    }

    waves
}

fn module_id(package_name: &str) -> String {
    package_name.to_owned()
}

fn affected_module_id(package_name: &str) -> String {
    format!("rust/{package_name}")
}

fn git_capture(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed in {}",
        args,
        repo.display()
    );
    String::from_utf8(output.stdout).expect("git output is utf-8")
}
