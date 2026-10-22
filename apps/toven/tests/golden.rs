//! The zero-code golden scenario suite.
//!
//! Every `scenario.yaml` under `tests/golden/` becomes one reported test —
//! adding coverage means dropping a scenario folder (definition + goldens)
//! under the tree, never editing this file. Discovery and execution live in
//! `toven_testkit::scenario`; this main only wires them to `libtest-mimic`.
//!
//! Check: `make golden`. Regenerate goldens: `make bless`. Set
//! `TOVEN_GOLDEN_REQUIRE_TOOLS=1` to fail scenarios skipped for a missing
//! toolchain (`make verify-release-platform` does).

use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Failed, Trial};
use toven_testkit::scenario::{Report, SkipPolicy, discover_scenarios, run_scenario};

fn main() {
    let args = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_toven"));
    let policy = SkipPolicy::from_env();

    let scenarios = match discover_scenarios(&root) {
        Ok(scenarios) => scenarios,
        Err(err) => {
            eprintln!("golden suite: discovery failed: {err}");
            std::process::exit(1);
        }
    };
    let trials: Vec<Trial> = scenarios
        .into_iter()
        .map(|dir| {
            let name = trial_name(&root, &dir);
            let binary = binary.clone();
            Trial::test(name, move || run_trial(&binary, &dir, policy))
        })
        .collect();

    libtest_mimic::run(&args, trials).exit();
}

/// One reported case per scenario, named by its folder relative to the root.
fn trial_name(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Run one scenario and map its report onto the test outcome under `policy`.
/// A skip (missing toolchain) passes unless the run requires every tool.
fn run_trial(binary: &Path, dir: &Path, policy: SkipPolicy) -> Result<(), Failed> {
    let report = run_scenario(binary, dir).map_err(|err| Failed::from(err.to_string()))?;
    if let Report::Skipped { tool } = &report
        && policy == SkipPolicy::Allow
    {
        eprintln!("skipped: requires `{tool}` which is not on PATH");
    }
    report
        .verdict(policy)
        .map_err(|message| Failed::from(format!("scenario {}: {message}", dir.display())))
}
