//! Binary-level smoke tests against real repositories.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use cargo_metadata::{DependencyKind, MetadataCommand, PackageId};

#[derive(Debug, serde::Deserialize)]
struct SmokeCase {
    name: Option<String>,
    repo: PathBuf,
    copy_root: Option<PathBuf>,
    task: Option<String>,
    args: Option<Vec<String>>,
    expected: PathBuf,
    config: Option<PathBuf>,
}

#[test]
fn managed_smoke_cases_match_expected_binary_output() {
    let root = manifest_dir();
    let selected = env::var("TOVEN_SMOKE_CASE").ok();
    let update = env::var("TOVEN_SMOKE_UPDATE").ok().as_deref() == Some("1");
    let cases = load_cases(&root);
    let mut matched = 0_usize;

    for case_path in cases {
        let case = load_case(&case_path);
        let case_name = case.name.clone().unwrap_or_else(|| {
            case_path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        });
        if selected
            .as_deref()
            .is_some_and(|selected| selected != case_name)
        {
            continue;
        }
        matched += 1;

        let source_repo = canonicalize(&root.join(&case.repo), &case_name);
        let (_fixture, repo) = isolated_repo(&root, &case, &case_name, &source_repo);
        let config = case_config(&root, &case, &case_name, &repo);
        let output = run_toven_plan(
            &config,
            case.task.as_deref().unwrap_or("test"),
            case.args.as_ref(),
        );
        if case.config.is_none() {
            let _ = fs::remove_file(&config);
        }
        let normalized = normalize_output(&output, &repo);
        assert_cargo_waves_match_output(&case_name, &repo, &normalized);
        let expected_path = root.join(&case.expected);

        if update {
            fs::create_dir_all(expected_path.parent().expect("expected has parent"))
                .expect("create expected directory");
            fs::write(&expected_path, &normalized).expect("write smoke expectation");
        } else {
            let expected = fs::read_to_string(&expected_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", expected_path.display()));
            assert_eq!(normalized, expected, "smoke case {case_name} changed");
        }
    }

    assert_ne!(matched, 0, "no smoke cases matched {selected:?}");
}

fn load_cases(root: &Path) -> Vec<PathBuf> {
    let cases_dir = root.join("smoke/cases");
    let mut cases = fs::read_dir(&cases_dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", cases_dir.display()))
        .map(|entry| entry.expect("read smoke case entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    cases.sort();
    cases
}

fn load_case(path: &Path) -> SmokeCase {
    let content =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    toml::from_str(&content).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn case_config(root: &Path, case: &SmokeCase, case_name: &str, repo: &Path) -> PathBuf {
    if let Some(config) = &case.config {
        return root.join(config);
    }

    let path = env::temp_dir().join(format!(
        "toven-smoke-{case_name}-{}.toml",
        std::process::id()
    ));
    fs::write(&path, generated_config(case_name, repo)).expect("write generated smoke config");
    path
}

fn isolated_repo(
    root: &Path,
    case: &SmokeCase,
    case_name: &str,
    source_repo: &Path,
) -> (rskit_fs::TempDir, PathBuf) {
    let fixture = rskit_fs::TempDir::new().expect("create smoke temp dir");
    let copy_source = case.copy_root.as_ref().map_or_else(
        || source_repo.to_path_buf(),
        |copy_root| root.join(copy_root),
    );
    let copy_source = canonicalize(&copy_source, case_name);
    let repo_relative = source_repo
        .strip_prefix(&copy_source)
        .unwrap_or_else(|error| {
            panic!(
                "smoke case {case_name} repo {} is not under copy root {}: {error}",
                source_repo.display(),
                copy_source.display()
            )
        });
    let dest = fixture.child("copy").expect("resolve smoke repo copy");
    rskit_fs::sync_io::tree::copy_tree(
        &copy_source,
        &dest,
        rskit_fs::sync_io::tree::CopyTreeOptions::default(),
    )
    .unwrap_or_else(|error| {
        panic!(
            "copy smoke root for {case_name} from {} to {}: {error}",
            copy_source.display(),
            dest.display()
        )
    });
    let repo = if repo_relative.as_os_str().is_empty() {
        dest
    } else {
        dest.join(repo_relative)
    };
    let repo = canonicalize(&repo, case_name);
    (fixture, repo)
}

fn generated_config(name: &str, repo: &Path) -> String {
    format!(
        r#"[workspace]
name = "{name}"
root = "{}"

[profiles.rust]
language = "rust"
execution = "batch-ready"
module_arg_template = ["-p", "{{module.package}}"]
resource_group = "cargo:{{workspace.root}}"

[profiles.rust.tasks]
test = {{ argv = ["cargo", "test", "{{module.args}}", "{{args}}"] }}
"#,
        repo.display()
    )
}

fn run_toven_plan(config: &Path, task: &str, args: Option<&Vec<String>>) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_toven"));
    command
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .arg("plan")
        .arg("--config")
        .arg(config)
        .arg("--task")
        .arg(task);

    if let Some(args) = args {
        command.arg("--").args(args);
    }

    let output = command.output().expect("run toven smoke binary");
    assert!(
        output.status.success(),
        "toven exited with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "toven wrote unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("toven stdout is utf-8")
}

fn normalize_output(output: &str, repo: &Path) -> String {
    let repo = repo.to_string_lossy().replace('\\', "/");
    output
        .replace("\r\n", "\n")
        .replace('\\', "/")
        .replace(&repo, "<WORKSPACE_ROOT>")
}

fn assert_cargo_waves_match_output(case_name: &str, repo: &Path, normalized_output: &str) {
    let expected = cargo_workspace_waves(repo);
    let actual = output_waves(normalized_output);
    assert_eq!(actual, expected, "smoke case {case_name} wave structure");
}

fn cargo_workspace_waves(repo: &Path) -> Vec<Vec<String>> {
    let metadata = MetadataCommand::new()
        .manifest_path(repo.join("Cargo.toml"))
        .current_dir(repo)
        .exec()
        .expect("read smoke repo cargo metadata");
    let workspace_ids = metadata
        .workspace_members
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.clone(), package))
        .collect::<BTreeMap<_, _>>();
    let nodes = metadata
        .resolve
        .as_ref()
        .expect("cargo metadata includes resolve graph")
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<BTreeMap<PackageId, _>>();
    let mut remaining = BTreeMap::new();

    for package in metadata.workspace_packages() {
        let mut dependencies = BTreeSet::new();
        if let Some(node) = nodes.get(&package.id) {
            for dependency in &node.deps {
                if workspace_ids.contains(&dependency.pkg) && !is_dev_only_dependency(dependency) {
                    dependencies.insert(packages[&dependency.pkg].name.to_string());
                }
            }
        }
        remaining.insert(package.name.to_string(), dependencies);
    }

    let mut satisfied = BTreeSet::new();
    let mut waves = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|(_, dependencies)| {
                dependencies
                    .iter()
                    .all(|dependency| satisfied.contains(dependency))
            })
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        assert!(
            !ready.is_empty(),
            "cargo workspace graph has a dependency cycle"
        );
        for name in &ready {
            satisfied.insert(name.clone());
            remaining.remove(name);
        }
        waves.push(ready);
    }

    waves
}

fn is_dev_only_dependency(dependency: &cargo_metadata::NodeDep) -> bool {
    !dependency.dep_kinds.is_empty()
        && dependency
            .dep_kinds
            .iter()
            .all(|kind| kind.kind == DependencyKind::Development)
}

fn output_waves(output: &str) -> Vec<Vec<String>> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("modules: "))
        .map(|modules| {
            modules
                .split(", ")
                .filter(|module| !module.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn canonicalize(path: &Path, case_name: &str) -> PathBuf {
    fs::canonicalize(path)
        .unwrap_or_else(|error| panic!("smoke case {case_name} repo {}: {error}", path.display()))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
