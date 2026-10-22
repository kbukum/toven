//! Standalone `toven-js` repo-driven smokes. js discovery is offline and
//! spawns no process, so the discovery smokes run without Node, a package
//! manager, or `node_modules`. The onboarding smokes drive a real package
//! manager and skip (green) with a reason when it or `node` is not on PATH.
//! The release smokes likewise drive a real package manager and skip without
//! it. The Yarn smokes also skip when `yarn` resolves to Yarn classic rather
//! than the fixture's pinned Yarn 4 (enable Corepack to provide it). Introspection projections render to **stdout**; the human run reporter
//! renders to **stderr**.

use std::path::{Path, PathBuf};
use std::process::Command;

use toven_testkit::{SampleRepo, program_on_path, run, run_ok};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_toven-js"))
}

fn repo(name: &str) -> SampleRepo {
    let repo = SampleRepo::materialize(name).expect("materialize fixture repo");
    repo.init_git().expect("init the fixture git tree");
    repo
}

#[test]
fn modules_lists_every_workspace_package() {
    let sample = repo("js/bun-workspace");
    run_ok(&binary(), sample.root(), &["modules"])
        .expect_stdout_contains("js:bun-workspace")
        .expect_stdout_contains("js:packages-std")
        .expect_stdout_contains("js:packages-web");
}

#[test]
fn graph_renders_the_workspace_edge() {
    let sample = repo("js/bun-workspace");
    run_ok(&binary(), sample.root(), &["graph"])
        .expect_stdout_contains("js:packages-web")
        .expect_stdout_contains("  -> js:packages-std");
}

#[test]
fn driver_list_links_js() {
    let sample = repo("js/bun-workspace");
    run_ok(&binary(), sample.root(), &["driver", "list"]).expect_stdout_contains("js");
}

/// Whether `yarn`, run in the Yarn 4 smoke fixture, is a modern Yarn (2+),
/// as Corepack provides from the fixture's `packageManager` pin. Logs why not.
fn modern_yarn_on_path() -> bool {
    if !program_on_path("yarn") {
        eprintln!("skipping: no `yarn` on PATH");
        return false;
    }
    let sample = SampleRepo::materialize("js/smoke-yarn-berry").expect("materialize fixture repo");
    let version = Command::new("yarn")
        .arg("--version")
        .current_dir(sample.root())
        .output()
        .expect("run yarn --version");
    let version = String::from_utf8_lossy(&version.stdout).trim().to_string();
    let modern = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u64>().ok())
        .is_some_and(|major| major >= 2);
    if !modern {
        eprintln!("skipping: `yarn` is {version}, not Yarn 4; enable Corepack");
    }
    modern
}

/// Onboard the workspace with `tool`, then prove the loop a user follows:
/// doctor reports the missing install, `install` runs the frozen install,
/// doctor passes, `test` builds the dependency before the test that imports
/// its output, and a second `test` replays from the cache. Returns `None`,
/// after logging why, when `node` or `tool` is not on PATH.
fn onboard_install_and_test(fixture: &str, tool: &str, install: &str) -> Option<SampleRepo> {
    for program in ["node", tool] {
        if !program_on_path(program) {
            eprintln!("skipping: no `{program}` on PATH");
            return None;
        }
    }
    let sample = repo(fixture);
    let root = sample.root();
    run_ok(&binary(), root, &["init"]).expect_stderr_contains("wrote ./toven.toml (js)");

    run(&binary(), root, &["doctor"])
        .expect_code(1)
        .expect_stderr_contains("dependencies are not installed")
        .expect_stderr_contains(&format!("fix: {install}"));

    run_ok(&binary(), root, &["install"]).expect_stderr_contains("ok js@js#install");
    run_ok(&binary(), root, &["doctor"]).expect_stderr_contains("check js/install: ok");

    run_ok(&binary(), root, &["test"])
        .expect_stderr_contains("ok js:packages-core#build")
        .expect_stderr_contains("ok js:packages-app#test");
    run_ok(&binary(), root, &["test"])
        .expect_stderr_contains("cached js:packages-core#build")
        .expect_stderr_contains("cached js:packages-app#test");
    Some(sample)
}

#[test]
fn npm_workspace_onboards_installs_and_tests() {
    onboard_install_and_test("js/smoke-npm", "npm", "npm ci");
}

#[test]
fn pnpm_workspace_onboards_installs_and_tests() {
    onboard_install_and_test("js/smoke-pnpm", "pnpm", "pnpm install --frozen-lockfile");
}

#[test]
fn yarn_berry_workspace_onboards_installs_and_tests() {
    if modern_yarn_on_path() {
        onboard_install_and_test("js/smoke-yarn-berry", "yarn", "yarn install --immutable");
    }
}

#[test]
fn bun_workspace_onboards_installs_and_tests() {
    let Some(sample) =
        onboard_install_and_test("js/smoke-bun", "bun", "bun install --frozen-lockfile")
    else {
        return;
    };
    // `bun test` in `app` measures core's built output as `../core/dist/index.js`,
    // relative to `app`; Toven credits that file to `core`, not to `app` or the
    // workspace root.
    run_ok(&binary(), sample.root(), &["coverage"])
        .expect_stderr_contains("ok js:packages-app#coverage")
        .expect_stderr_contains("coverage js:packages-core: passed (line 100.00%")
        .expect_stderr_not_contains("coverage js:packages-app:")
        .expect_stderr_not_contains("coverage js:acme-smoke:");
}

/// Release config for the smoke workspaces: npm-registry publication with
/// network-free idempotency, and the versionless root excluded.
const RELEASE_CONFIG: &str = r#"[project]
name = "acme-smoke"

[ecosystems.js.release]
registry = "npmjs"
offline = true
push = false

[modules."js:acme-smoke".release]
exclude = true
"#;

/// How a smoke workspace drives a release bump with its package manager.
struct BumpCase<'a> {
    fixture: &'a str,
    tool: &'a str,
    /// `app`'s spec on `core` before and after `core` moves to 2.0.0.
    spec: (&'a str, &'a str),
    /// Syncs the lockfile with the new spec, without running scripts.
    sync: &'a [&'a str],
    /// Installs exactly the lockfile, failing when it is stale.
    frozen: &'a [&'a str],
    /// A lockfile that must now record the new version, when the manager
    /// records workspace versions in it.
    recorded: Option<&'a str>,
}

/// Prove a release bump leaves the lockfile frozen-install clean: give `app`
/// its spec on `core`, sync the lockfile, then major-bump `core`. The bump
/// rewrites a simple range (keeping a `workspace:` spec, which the manager
/// resolves at pack time) and refreshes the lockfile, and the frozen install
/// still passes. Skips (green) when the tool is not on PATH.
fn bump_keeps_frozen_install_clean(case: &BumpCase<'_>) {
    if !program_on_path(case.tool) {
        eprintln!("skipping: no `{}` on PATH", case.tool);
        return;
    }
    let (before, after) = case.spec;
    let sample = SampleRepo::materialize(case.fixture).expect("materialize fixture repo");
    let root = sample.root();
    let app = sample.child("packages/app/package.json");
    let manifest = std::fs::read_to_string(&app).expect("read app manifest");
    let manifest = [
        r#""*""#,
        r#""workspace:*""#,
        r#""workspace:^""#,
        r#""^1.0.0""#,
    ]
    .iter()
    .fold(manifest, |text, spec| {
        text.replace(
            &format!(r#""@acme/core": {spec}"#),
            &format!(r#""@acme/core": "{before}""#),
        )
    });
    std::fs::write(&app, manifest).expect("write app manifest");
    std::fs::write(sample.child("toven.toml"), RELEASE_CONFIG).expect("write toven.toml");
    run_ok(Path::new(case.tool), root, case.sync);
    sample.init_git().expect("init the fixture git tree");

    run_ok(
        &binary(),
        root,
        &[
            "release",
            "bump",
            "--yes",
            "--set-version",
            "js:packages-core=2.0.0",
        ],
    )
    .expect_stderr_contains("release js:packages-core: staged 2.0.0");

    let staged = std::fs::read_to_string(&app).expect("read app manifest");
    assert!(
        staged.contains(&format!(r#""@acme/core": "{after}""#)),
        "{staged}"
    );
    let core = std::fs::read_to_string(sample.child("packages/core/package.json"))
        .expect("read core manifest");
    assert!(core.contains(r#""version": "2.0.0""#), "{core}");
    if let Some(lockfile) = case.recorded {
        let locked = std::fs::read_to_string(sample.child(lockfile)).expect("read lockfile");
        assert!(locked.contains(r#""version": "2.0.0""#), "{locked}");
        assert!(locked.contains(r#""@acme/core": "^2.0.0""#), "{locked}");
    }
    run_ok(Path::new(case.tool), root, case.frozen);
}

#[test]
fn npm_bump_keeps_frozen_install_clean() {
    bump_keeps_frozen_install_clean(&BumpCase {
        fixture: "js/smoke-npm",
        tool: "npm",
        spec: ("^1.0.0", "^2.0.0"),
        sync: &["install", "--package-lock-only", "--ignore-scripts"],
        frozen: &["ci", "--ignore-scripts"],
        recorded: Some("package-lock.json"),
    });
}

#[test]
fn bun_bump_keeps_frozen_install_clean() {
    // Bun keeps workspace metadata it does not need to resolve, so its
    // lockfile records no version to check.
    bump_keeps_frozen_install_clean(&BumpCase {
        fixture: "js/smoke-bun",
        tool: "bun",
        spec: ("^1.0.0", "^2.0.0"),
        sync: &["install", "--lockfile-only", "--ignore-scripts"],
        frozen: &["install", "--frozen-lockfile", "--ignore-scripts"],
        recorded: None,
    });
}

#[test]
fn pnpm_bump_keeps_frozen_install_clean() {
    bump_keeps_frozen_install_clean(&BumpCase {
        fixture: "js/smoke-pnpm",
        tool: "pnpm",
        spec: ("workspace:^", "workspace:^"),
        sync: &["install", "--lockfile-only", "--ignore-scripts"],
        frozen: &["install", "--frozen-lockfile", "--ignore-scripts"],
        recorded: None,
    });
}

#[test]
fn yarn_berry_bump_keeps_frozen_install_clean() {
    // Yarn records workspace versions as `0.0.0-use.local`, but keys entries
    // by range, so `--immutable` fails unless the bump refreshed the lockfile.
    if modern_yarn_on_path() {
        bump_keeps_frozen_install_clean(&BumpCase {
            fixture: "js/smoke-yarn-berry",
            tool: "yarn",
            spec: ("^1.0.0", "^2.0.0"),
            sync: &["install", "--mode=update-lockfile"],
            frozen: &["install", "--immutable", "--mode=skip-build"],
            recorded: None,
        });
    }
}
