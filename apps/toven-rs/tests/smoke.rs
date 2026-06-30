//! End-to-end smoke for the standalone `toven-rs` app binary.
//!
//! Materializes the committed `single-rust` fixture into a throwaway git tree
//! and drives the real binary across the argv-first surface: an introspection
//! projection (`modules`), a PLAN-only cut (`plan build`), and a full PLAN+APPLY
//! run (`build`). The whole standalone stack is exercised as a shipping binary;
//! each command must exit zero. Fixture
//! materialization and the real git import reuse `toven-testkit` primitives
//! rather than an out-of-band shell harness.

use std::path::Path;
use std::process::Command;

use toven_testkit::SampleRepo;

/// Path to the freshly-built `toven-rs` app binary under test.
const fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_toven-rs")
}

/// Run `toven-rs <args>` in `cwd`, asserting a zero exit and returning stdout.
fn run(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new(binary())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn the toven-rs binary");
    assert!(
        output.status.success(),
        "`toven-rs {}` exited with {:?}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("toven-rs stdout is valid UTF-8")
}

#[test]
fn standalone_rust_app_plans_and_applies_a_real_fixture() {
    let repo = SampleRepo::materialize("single-rust").expect("materialize single-rust fixture");
    repo.init_git().expect("init the fixture git tree");
    let root = repo.root();

    let modules = run(root, &["modules"]);
    assert!(
        modules.contains("rust:app"),
        "`modules` should list the rust:app module, got:\n{modules}",
    );

    // PLAN-only cut, then the full PLAN+APPLY run (which shells out to cargo).
    run(root, &["plan", "build"]);
    run(root, &["build"]);
}
