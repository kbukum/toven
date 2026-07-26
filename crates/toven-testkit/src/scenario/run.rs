use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::{dir, file};
use rskit_testutil::{Golden, GoldenMode, GoldenOutcome};

use crate::exec;
use crate::git::GitScenario;
use crate::repo::SampleRepo;

use super::effects::{self, EffectContext};
use super::matcher_kind::NormalizeScope;
use super::model::{GitScript, Scenario, Step};
use super::report::{Report, StepOutcome, StepStatus};

/// Environment variable that points the CLI's cache at a directory.
///
/// Mirrors `toven_engine::cache::root::CACHE_DIR_ENV` (kept as a literal here
/// because the dev-only testkit does not depend on the engine crate). The
/// engine sets it to a scenario-scoped directory so sessions never share cache
/// state — with each other or with the developer's real cache.
pub const CACHE_DIR_ENV: &str = "TOVEN_CACHE_DIR";

/// Base epoch second for scripted git history; commit `N` is pinned to
/// `base + N * 60` so history is strictly ordered yet byte-stable. The same
/// instant as [`exec::CLOCK_EPOCH_VALUE`] — one pinned "now" for the whole
/// deterministic session; keep the two in step when changing either.
const GIT_EPOCH_BASE: u64 = 1_700_000_000;

/// The pinned timestamp of scripted commit `index` (0 = the import commit).
fn commit_epoch(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(GIT_EPOCH_BASE + (index as u64) * 60)
}

/// Run the scenario in `dir` against `binary`, with the golden mode taken
/// from the environment and toolchains probed on `PATH`.
///
/// # Errors
///
/// See [`run_scenario_with`].
pub fn run_scenario(binary: &Path, dir: &Path) -> AppResult<Report> {
    run_scenario_with(binary, dir, GoldenMode::from_env(), exec::program_on_path)
}

/// Run the scenario in `dir` against `binary` with an explicit golden `mode`
/// and toolchain probe.
///
/// The session: load → toolchain gate → materialize the fixture repo →
/// `git init` + pinned import commit → apply the git script → run each step
/// in-repo (deterministic env, scenario-scoped cache), verifying streams and
/// effects. Execution stops at the first failed step.
///
/// # Errors
///
/// Returns a typed [`AppError`] for infrastructure problems (a malformed
/// scenario, a missing fixture repo, an unspawnable binary). Assertion drifts
/// (exit codes, golden mismatches, effects) are reported as
/// [`StepStatus::Failed`] outcomes, not errors, so the harness renders them
/// per step. One deliberate blur: a golden *write* failure in bless mode also
/// surfaces as the step's `Failed` outcome (it reaches the caller through the
/// same per-stream verify path).
pub fn run_scenario_with(
    binary: &Path,
    dir: &Path,
    mode: GoldenMode,
    toolchain_present: impl Fn(&str) -> bool,
) -> AppResult<Report> {
    let scenario = Scenario::load(dir)?;
    if let Some(missing) = scenario
        .requires
        .iter()
        .find(|tool| !toolchain_present(tool.program()))
    {
        return Ok(Report::Skipped {
            tool: missing.program().to_owned(),
        });
    }

    let repo = SampleRepo::materialize(&scenario.repo)?;
    let git = GitScenario::init(repo.root())?;
    git.commit_all_pinned("import fixture repo", commit_epoch(0))?;
    if let Some(script) = &scenario.git {
        apply_git_script(&git, script)?;
    }

    let cache_dir = repo.workspace().child("cache")?;
    dir::create_all(&cache_dir)?;
    // A scenario-scoped home for real toolchains (cargo/go), kept *outside* the
    // toven cache dir so the `cache_entries` effect never counts it. Isolating
    // it per scenario removes the shared package-cache lock so concurrent
    // real-toolchain scenarios don't emit "Blocking waiting for file lock…"
    // noise under the harness's parallelism.
    let tool_home = repo.workspace().child("toolchains")?;
    dir::create_all(&tool_home)?;
    // Canonicalize so the normalizer's literal rules match what the spawned
    // binary sees from `getcwd` (macOS temp dirs resolve /var → /private/var).
    let repo_root = rskit_fs::canonicalize(repo.root())?;
    let cache_dir = rskit_fs::canonicalize(&cache_dir)?;
    let tool_home = rskit_fs::canonicalize(&tool_home)?;
    let scope = NormalizeScope {
        repo_root: repo_root.clone(),
        cache_dir: cache_dir.clone(),
    };
    let env = step_env(&scenario, &cache_dir, &tool_home);
    let cx = StepContext {
        binary,
        scenario_dir: dir,
        repo_root: &repo_root,
        cache_dir: &cache_dir,
        scope: &scope,
        env: &env,
        mode,
    };

    let mut steps = Vec::with_capacity(scenario.steps.len());
    for step in &scenario.steps {
        let status = match step
            .requires
            .iter()
            .find(|tool| !toolchain_present(tool.program()))
        {
            Some(missing) => StepStatus::Skipped {
                tool: missing.program().to_owned(),
            },
            None => run_step(&cx, step)?,
        };
        let failed = matches!(status, StepStatus::Failed { .. });
        steps.push(StepOutcome {
            id: step.id.clone(),
            status,
        });
        if failed {
            break;
        }
    }
    Ok(Report::Completed { steps })
}

/// The per-session invariants every step run shares.
struct StepContext<'a> {
    binary: &'a Path,
    scenario_dir: &'a Path,
    repo_root: &'a Path,
    cache_dir: &'a Path,
    scope: &'a NormalizeScope,
    env: &'a BTreeMap<String, String>,
    mode: GoldenMode,
}

/// Apply a scenario git script deterministically: each commit touches its
/// files then commits with a pinned signature; branches and lightweight tags
/// are created at the final `HEAD`.
///
/// # Errors
///
/// Returns a typed [`AppError`] on a traversing touch path or any git failure.
pub fn apply_git_script(git: &GitScenario, script: &GitScript) -> AppResult<()> {
    for (index, commit) in script.commits.iter().enumerate() {
        for rel in &commit.touch {
            touch(git.root(), rel)?;
        }
        // Index 0 is the import commit, so scripted commits start at 1.
        git.commit_all_pinned(&commit.msg, commit_epoch(index + 1))?;
    }
    for branch in &script.branches {
        git.branch(branch)?;
    }
    for tag in &script.tags {
        git.tag_lightweight(tag)?;
    }
    Ok(())
}

/// Create or deterministically append to a repo-relative file so the next
/// commit sees a change.
fn touch(repo_root: &Path, rel: &str) -> AppResult<()> {
    let path = safe_join(repo_root, rel)
        .map_err(|err| AppError::invalid_input("git touch path", err.to_string()))?;
    let mut contents = if file::exists(&path)? {
        file::read_string(&path)?
    } else {
        file::create_parent_dir(&path)?;
        String::new()
    };
    contents.push_str("touched by scenario git script\n");
    file::write(&path, contents)
}

/// The deterministic environment overlay for every step: pinned clock, plain
/// locale/terminal, the scoped cache, and per-scenario toolchain homes — with
/// the scenario's own `env` on top.
fn step_env(scenario: &Scenario, cache_dir: &Path, tool_home: &Path) -> BTreeMap<String, String> {
    let cargo_home = tool_home.join("cargo");
    let go_cache = tool_home.join("go-cache");
    let go_path = tool_home.join("go-path");
    let mut env = BTreeMap::from([
        (
            exec::CLOCK_EPOCH_ENV.to_owned(),
            exec::CLOCK_EPOCH_VALUE.to_owned(),
        ),
        (CACHE_DIR_ENV.to_owned(), cache_dir.display().to_string()),
        ("LC_ALL".to_owned(), "C".to_owned()),
        ("TERM".to_owned(), "dumb".to_owned()),
        // Isolate real-toolchain caches per scenario so parallel cargo/go steps
        // never contend on a shared package-cache lock (which would inject
        // nondeterministic "Blocking waiting for file lock…" lines).
        ("CARGO_HOME".to_owned(), cargo_home.display().to_string()),
        ("GOCACHE".to_owned(), go_cache.display().to_string()),
        ("GOPATH".to_owned(), go_path.display().to_string()),
        (
            "GOMODCACHE".to_owned(),
            go_path.join("pkg/mod").display().to_string(),
        ),
    ]);
    env.extend(
        scenario
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    env
}

/// Run one step: spawn, assert exit, verify/bless streams, evaluate effects.
fn run_step(cx: &StepContext<'_>, step: &Step) -> AppResult<StepStatus> {
    // User argv verbatim; only the declared config variant is appended.
    let mut argv = step.argv.clone();
    if let Some(config) = &step.config {
        argv.push("--config".to_owned());
        argv.push(config.clone());
    }
    let capture = exec::capture(cx.binary, cx.repo_root, &argv, cx.env)?;

    if capture.code != Some(step.exit) {
        return Ok(StepStatus::Failed {
            message: format!(
                "step '{}': expected exit {}, got {:?}\nstdout:\n{}\nstderr:\n{}",
                step.id, step.exit, capture.code, capture.stdout, capture.stderr
            ),
        });
    }

    let mut blessed = false;
    let streams = [
        ("stdout", &step.stdout, &capture.stdout),
        ("stderr", &step.stderr, &capture.stderr),
    ];
    for (stream, expectation, actual) in streams {
        let Some(expectation) = expectation else {
            continue;
        };
        let matcher = expectation.to_match(cx.scope)?;
        let golden = Golden::new(golden_path(cx.scenario_dir, &step.id, stream), matcher);
        match golden.run(actual, cx.mode) {
            Ok(GoldenOutcome::Blessed) => blessed = true,
            Ok(GoldenOutcome::Verified) => {}
            Err(err) => {
                return Ok(StepStatus::Failed {
                    message: format!("step '{}' {stream}: {err}", step.id),
                });
            }
        }
    }

    let effect_cx = EffectContext {
        repo_root: cx.repo_root,
        cache_dir: cx.cache_dir,
        scenario_dir: cx.scenario_dir,
        mode: cx.mode,
    };
    match effects::check(step, &effect_cx) {
        Ok(effects_blessed) => blessed |= effects_blessed,
        Err(err) => {
            return Ok(StepStatus::Failed {
                message: err.to_string(),
            });
        }
    }
    Ok(if blessed {
        StepStatus::Blessed
    } else {
        StepStatus::Passed
    })
}

/// The golden file for one step stream: `<scenario dir>/<step id>.<stream>`.
fn golden_path(scenario_dir: &Path, step_id: &str, stream: &str) -> PathBuf {
    scenario_dir.join(format!("{step_id}.{stream}"))
}
