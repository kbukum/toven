//! Binary-level smoke tests against real repositories.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use cargo_metadata::{DependencyKind, MetadataCommand, PackageId};

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeCase {
    name: Option<String>,
    repo: PathBuf,
    copy_root: Option<PathBuf>,
    #[serde(default)]
    command: SmokeCommand,
    #[serde(default)]
    affected: bool,
    base: Option<String>,
    #[serde(default)]
    merge_base: bool,
    branch: Option<String>,
    changes: Option<Vec<SmokeChange>>,
    task: Option<String>,
    args: Option<Vec<String>>,
    expected: PathBuf,
    config: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SmokeCommand {
    #[default]
    Plan,
    Affected,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeChange {
    path: PathBuf,
    #[serde(default)]
    mode: SmokeChangeMode,
    content: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SmokeChangeMode {
    #[default]
    Append,
    Write,
}

#[test]
fn managed_smoke_cases_match_expected_binary_output() {
    if env::var("TOVEN_SMOKE_SKIP_MANAGED").ok().as_deref() == Some("1") {
        return;
    }

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
        let (fixture, repo) = isolated_repo(&root, &case, &case_name, &source_repo);
        prepare_git_fixture(&case, &case_name, &repo);
        let config = case_config(&root, &case, &case_name, &repo, &fixture);
        let output = run_toven(
            &case,
            &config,
            case.task.as_deref().unwrap_or("test"),
            case.args.as_ref(),
        );
        let normalized = normalize_output(&output, &repo);
        if case.affected || matches!(case.command, SmokeCommand::Affected) {
            assert_affected_modules_match_metadata(&case_name, &repo, &case, &normalized);
        } else {
            assert_cargo_waves_match_output(&case_name, &repo, &normalized);
        }
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

fn case_config(
    root: &Path,
    case: &SmokeCase,
    case_name: &str,
    repo: &Path,
    fixture: &rskit_fs::TempDir,
) -> PathBuf {
    if let Some(config) = &case.config {
        return isolated_case_config(root, config, case_name, repo, fixture);
    }

    fixture
        .write_file(
            "generated-toven.toml",
            generated_config(case_name, repo).as_bytes(),
        )
        .expect("write generated smoke config")
}

fn isolated_case_config(
    root: &Path,
    config: &Path,
    case_name: &str,
    repo: &Path,
    fixture: &rskit_fs::TempDir,
) -> PathBuf {
    let source = root.join(config);
    let content = fs::read_to_string(&source)
        .unwrap_or_else(|error| panic!("read smoke config {}: {error}", source.display()));
    let mut document = toml::from_str::<toml::Table>(&content)
        .unwrap_or_else(|error| panic!("parse smoke config {}: {error}", source.display()));
    let workspace = document
        .entry("workspace")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let workspace = workspace.as_table_mut().unwrap_or_else(|| {
        panic!(
            "smoke config {} workspace must be a TOML table",
            source.display()
        )
    });
    workspace.insert(
        "root".to_string(),
        toml::Value::String(repo.to_string_lossy().into_owned()),
    );

    fixture
        .write_file("isolated-toven.toml", document.to_string().as_bytes())
        .unwrap_or_else(|error| panic!("write isolated smoke config for {case_name}: {error}"))
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
    let name = toml_string(name);
    let repo = toml_string(repo.to_str().expect("smoke repo path is utf-8"));
    format!(
        r#"[workspace]
name = {name}
root = {repo}

[profiles.rust]
language = "rust"
execution = "batch-ready"
module_arg_template = ["-p", "{{module.package}}"]
resource_group = "cargo:{{workspace.root}}"

[profiles.rust.tasks]
test = {{ argv = ["cargo", "test", "{{module.args}}", "{{args}}"] }}
"#
    )
}

fn toml_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            character if character.is_control() => {
                write!(escaped, "\\u{:04X}", character as u32).expect("write escaped character");
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn prepare_git_fixture(case: &SmokeCase, case_name: &str, repo: &Path) {
    if !case.needs_git_fixture() {
        return;
    }

    remove_copied_git_metadata(repo, case_name);
    run_git(
        repo,
        case_name,
        &["init", "--initial-branch=main", "--quiet"],
    );
    run_git(repo, case_name, &["config", "user.name", "Toven Smoke"]);
    run_git(
        repo,
        case_name,
        &["config", "user.email", "toven-smoke@example.invalid"],
    );
    run_git(repo, case_name, &["add", "-A"]);
    run_git(repo, case_name, &["commit", "--quiet", "-m", "baseline"]);

    if let Some(branch) = &case.branch {
        run_git(repo, case_name, &["checkout", "--quiet", "-b", branch]);
    }
    apply_changes(repo, case_name, case.changes.as_deref().unwrap_or(&[]));
    if case.branch.is_some() {
        run_git(repo, case_name, &["add", "-A"]);
        run_git(
            repo,
            case_name,
            &["commit", "--quiet", "-m", "affected changes"],
        );
    }
}

impl SmokeCase {
    fn needs_git_fixture(&self) -> bool {
        self.affected
            || matches!(self.command, SmokeCommand::Affected)
            || self.base.is_some()
            || self.branch.is_some()
            || self
                .changes
                .as_ref()
                .is_some_and(|changes| !changes.is_empty())
    }
}

fn remove_copied_git_metadata(repo: &Path, case_name: &str) {
    let git = repo.join(".git");
    let Ok(metadata) = fs::symlink_metadata(&git) else {
        return;
    };
    if metadata.is_dir() {
        fs::remove_dir_all(&git)
            .unwrap_or_else(|error| panic!("remove copied .git for {case_name}: {error}"));
    } else {
        fs::remove_file(&git)
            .unwrap_or_else(|error| panic!("remove copied .git for {case_name}: {error}"));
    }
}

fn apply_changes(repo: &Path, case_name: &str, changes: &[SmokeChange]) {
    for change in changes {
        let target = repo.join(&change.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!(
                    "create smoke change parent {} for {case_name}: {error}",
                    parent.display()
                )
            });
        }
        match change.mode {
            SmokeChangeMode::Append => {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&target)
                    .unwrap_or_else(|error| {
                        panic!(
                            "open smoke change target {} for {case_name}: {error}",
                            target.display()
                        )
                    });
                file.write_all(change.content.as_bytes())
                    .unwrap_or_else(|error| {
                        panic!(
                            "append smoke change target {} for {case_name}: {error}",
                            target.display()
                        )
                    });
            }
            SmokeChangeMode::Write => {
                fs::write(&target, &change.content).unwrap_or_else(|error| {
                    panic!(
                        "write smoke change target {} for {case_name}: {error}",
                        target.display()
                    )
                });
            }
        }
    }
}

fn run_git(repo: &Path, case_name: &str, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2001-02-03T04:05:06Z")
        .env("GIT_COMMITTER_DATE", "2001-02-03T04:05:06Z")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "gc.auto=0",
        ])
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run git for smoke case {case_name}: {error}"));
    assert!(
        output.status.success(),
        "git {:?} failed for smoke case {case_name} with {}\nstdout:\n{}\nstderr:\n{}",
        args,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_toven(case: &SmokeCase, config: &Path, task: &str, args: Option<&Vec<String>>) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_toven"));
    command
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .arg(match case.command {
            SmokeCommand::Plan => "plan",
            SmokeCommand::Affected => "affected",
        })
        .arg("--config")
        .arg(config)
        .arg("--task")
        .arg(task);

    if case.affected {
        command.arg("--affected");
    }
    if let Some(base) = &case.base {
        command.arg("--base").arg(base);
    }
    if case.merge_base {
        command.arg("--merge-base");
    }
    if matches!(case.command, SmokeCommand::Plan)
        && let Some(args) = args
    {
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
        .lines()
        .map(normalize_git_oid_line)
        .collect::<Vec<_>>()
        .join("\n")
        + if output.ends_with('\n') { "\n" } else { "" }
}

fn normalize_git_oid_line(line: &str) -> String {
    let Some(rest) = line.strip_prefix("baseline: ") else {
        return line.to_string();
    };
    let Some((provider, oid)) = rest.rsplit_once(' ') else {
        return line.to_string();
    };
    if oid.len() == 40 && oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        format!("baseline: {provider} <OID>")
    } else {
        line.to_string()
    }
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

fn assert_affected_modules_match_metadata(
    case_name: &str,
    repo: &Path,
    case: &SmokeCase,
    normalized_output: &str,
) {
    let expected = expected_affected_modules(repo, case.changes.as_deref().unwrap_or(&[]));
    let actual = output_module_set(normalized_output);
    assert_eq!(
        actual, expected,
        "smoke case {case_name} affected module set"
    );
}

fn expected_affected_modules(repo: &Path, changes: &[SmokeChange]) -> BTreeSet<String> {
    if changes.is_empty() {
        return BTreeSet::new();
    }

    let metadata = MetadataCommand::new()
        .manifest_path(repo.join("Cargo.toml"))
        .current_dir(repo)
        .exec()
        .expect("read smoke repo cargo metadata");
    let all = metadata
        .workspace_packages()
        .iter()
        .map(|package| package.name.to_string())
        .collect::<BTreeSet<_>>();
    let mut direct = BTreeSet::new();

    for change in changes {
        if change.path.components().count() == 1 {
            return all;
        }

        let changed_path = repo.join(&change.path);
        let mut owner = None;
        for package in metadata.workspace_packages() {
            let package_root = package
                .manifest_path
                .parent()
                .expect("package manifest has parent")
                .as_std_path();
            if changed_path.starts_with(package_root)
                && owner.as_ref().is_none_or(|owner: &(String, PathBuf)| {
                    let owner_root = &owner.1;
                    package_root.components().count() > owner_root.components().count()
                })
            {
                owner = Some((package.name.to_string(), package_root.to_path_buf()));
            }
        }

        let Some((owner, _)) = owner else {
            return all;
        };
        direct.insert(owner);
    }

    reverse_dependent_closure(&metadata, direct)
}

fn reverse_dependent_closure(
    metadata: &cargo_metadata::Metadata,
    mut affected: BTreeSet<String>,
) -> BTreeSet<String> {
    let workspace_roots = metadata
        .workspace_packages()
        .iter()
        .map(|package| {
            (
                package
                    .manifest_path
                    .parent()
                    .expect("package manifest has parent")
                    .as_std_path()
                    .to_path_buf(),
                package.name.to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let workspace_names = metadata
        .workspace_packages()
        .iter()
        .map(|package| package.name.to_string())
        .collect::<BTreeSet<_>>();
    let mut reverse = BTreeMap::<String, BTreeSet<String>>::new();

    for package in metadata.workspace_packages() {
        for dependency in &package.dependencies {
            let dependency_name = dependency
                .path
                .as_ref()
                .and_then(|path| workspace_roots.get(path.as_std_path()).cloned())
                .or_else(|| workspace_names.get(dependency.name.as_str()).cloned());
            if let Some(dependency_name) = dependency_name {
                reverse
                    .entry(dependency_name)
                    .or_default()
                    .insert(package.name.to_string());
            }
        }
    }

    let mut queue = affected.iter().cloned().collect::<Vec<_>>();
    while let Some(module) = queue.pop() {
        for dependent in reverse.get(&module).into_iter().flatten() {
            if affected.insert(dependent.clone()) {
                queue.push(dependent.clone());
            }
        }
    }

    affected
}

fn output_module_set(output: &str) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let mut in_affected_modules = false;
    for line in output.lines() {
        if let Some(batch_modules) = line.strip_prefix("modules: ") {
            modules.extend(
                batch_modules
                    .split(", ")
                    .filter(|module| !module.is_empty())
                    .map(ToString::to_string),
            );
        } else if line == "modules:" {
            in_affected_modules = true;
        } else if in_affected_modules && (line == "changed_paths:" || line.is_empty()) {
            in_affected_modules = false;
        } else if in_affected_modules && let Some(module) = line.strip_prefix("- ") {
            let module = module.split_once(" (").map_or(module, |(module, _)| module);
            modules.insert(module.to_string());
        } else if let Some(module) = line.strip_prefix("- ") {
            debug_assert!(
                !module.contains(" (direct)") && !module.contains(" (dependent)"),
                "module-like affected line was not parsed in a modules section: {line}"
            );
        }
    }
    modules
}

fn canonicalize(path: &Path, case_name: &str) -> PathBuf {
    fs::canonicalize(path)
        .unwrap_or_else(|error| panic!("smoke case {case_name} repo {}: {error}", path.display()))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn generated_config_escapes_toml_strings() {
    let repo = Path::new("/tmp/repo\"\\\t");
    let config = generated_config("case\"\\\n", repo);
    let parsed = toml::from_str::<toml::Value>(&config).expect("generated config is valid TOML");

    assert_eq!(parsed["workspace"]["name"].as_str(), Some("case\"\\\n"));
    assert_eq!(
        parsed["workspace"]["root"].as_str(),
        Some("/tmp/repo\"\\\t")
    );
}

#[test]
fn smoke_case_rejects_unknown_fields() {
    let error = toml::from_str::<SmokeCase>(
        r#"
repo = "repo"
expected = "expected.plan.txt"
argz = []
"#,
    )
    .expect_err("unknown smoke case fields should fail parsing");

    assert!(
        error.to_string().contains("unknown field"),
        "unexpected parse error: {error}"
    );
}

#[test]
fn custom_case_config_points_workspace_root_at_isolated_repo() {
    let root = rskit_fs::TempDir::new().expect("create root temp dir");
    let fixture = rskit_fs::TempDir::new().expect("create fixture temp dir");
    let repo = fixture.child("copy").expect("resolve isolated repo");
    fs::create_dir_all(&repo).expect("create isolated repo");
    root.write_file(
        "source/toven.toml",
        br#"
[workspace]
name = "demo"
root = "."
"#,
    )
    .expect("write source config");
    let case = SmokeCase {
        name: Some("custom".to_string()),
        repo: PathBuf::from("source"),
        copy_root: None,
        command: SmokeCommand::Plan,
        affected: false,
        base: None,
        merge_base: false,
        branch: None,
        changes: None,
        task: None,
        args: None,
        expected: PathBuf::from("expected.plan.txt"),
        config: Some(PathBuf::from("source/toven.toml")),
    };

    let config = case_config(root.path(), &case, "custom", &repo, &fixture);
    let config_content = fs::read_to_string(config).expect("read isolated config");
    let parsed = toml::from_str::<toml::Table>(&config_content).expect("isolated config is TOML");

    assert_eq!(parsed["workspace"]["name"].as_str(), Some("demo"));
    assert_eq!(
        parsed["workspace"]["root"].as_str(),
        Some(repo.to_string_lossy().as_ref())
    );
}
