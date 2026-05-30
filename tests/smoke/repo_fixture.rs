use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rskit_testutil::TestWorkspace;

use crate::case::{SmokeCase, SmokeSetup};

pub struct PreparedSmokeRepo {
    pub workspace: TestWorkspace,
    pub repo: PathBuf,
    pub config: PathBuf,
}

pub fn prepare(root: &Path, case: &SmokeCase, case_name: &str) -> PreparedSmokeRepo {
    let temp = TestWorkspace::new(case_name);
    let repo = temp.path().join("repo");
    let source = case_source(root, case);
    let source = case
        .copy_root
        .as_ref()
        .map_or_else(|| source.clone(), |copy_root| source.join(copy_root));
    copy_dir_recursive(&source, &repo);
    let repo = fs::canonicalize(&repo).expect("canonicalize smoke fixture repo");

    let config = case.config.as_ref().map_or_else(
        || {
            let fixture_config = repo.join("toven.toml");
            if fixture_config.exists() {
                fixture_config
            } else {
                let config = generated_config(&repo);
                let config_path = temp.path().join("toven.toml");
                fs::write(&config_path, config).expect("write generated smoke config");
                config_path
            }
        },
        |config| {
            let config_path = temp.path().join("toven.toml");
            fs::write(
                &config_path,
                toml::to_string_pretty(config).expect("serialize generated smoke config"),
            )
            .expect("write generated smoke config");
            config_path
        },
    );

    for setup in &case.setup {
        apply_setup(&repo, setup);
    }

    if case.needs_git_fixture() {
        git(&repo, &["init", "--initial-branch", "main"]);
        git(&repo, &["config", "user.email", "smoke@example.com"]);
        git(&repo, &["config", "user.name", "Smoke Tests"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);

        if let Some(branch) = &case.branch {
            git(&repo, &["switch", "-c", branch]);
        }

        for change in &case.changes {
            let path = repo.join(&change.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create change parent");
            }
            fs::write(path, &change.content).expect("write changed file");
        }
    }

    PreparedSmokeRepo {
        workspace: temp,
        repo,
        config,
    }
}

fn case_source(root: &Path, case: &SmokeCase) -> PathBuf {
    if let Some(fixture) = &case.fixture {
        return root.join("smoke/fixtures").join(fixture);
    }

    root.join(case.repo.as_ref().expect("case has repo or fixture"))
}

fn apply_setup(repo: &Path, setup: &SmokeSetup) {
    match setup {
        SmokeSetup::Write { path, content } => {
            let path = repo.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create setup parent");
            }
            fs::write(path, content).expect("write setup file");
        }
        SmokeSetup::Append { path, content } => {
            let path = repo.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create setup parent");
            }
            let mut existing = fs::read_to_string(&path).unwrap_or_default();
            existing.push_str(content);
            fs::write(path, existing).expect("append setup file");
        }
        SmokeSetup::Delete { path } => {
            let path = repo.join(path);
            if path.is_dir() {
                fs::remove_dir_all(path).expect("delete setup directory");
            } else if path.exists() {
                fs::remove_file(path).expect("delete setup file");
            }
        }
    }
}

fn generated_config(repo: &Path) -> String {
    format!(
        r#"[workspace]
name = "generated-smoke"
root = "{}"

[profiles.rust]
language = "rust"
module_arg_template = ["-p", "{{module.package}}"]

[profiles.rust.tasks]
test = {{ argv = ["cargo", "check", "-q", "{{module.args}}"] }}
"#,
        repo.display()
    )
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
        args,
        repo.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_dir_recursive(source: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("create fixture destination");
    for entry in fs::read_dir(source).expect("read fixture source") {
        let entry = entry.expect("read fixture source entry");
        let entry_source = entry.path();
        let entry_dest = dest.join(entry.file_name());
        if entry.file_type().expect("read fixture entry type").is_dir() {
            copy_dir_recursive(&entry_source, &entry_dest);
        } else {
            fs::copy(&entry_source, &entry_dest).expect("copy fixture file");
        }
    }
}
