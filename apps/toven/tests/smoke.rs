//! End-to-end smoke for the umbrella `toven` app binary.
//!
//! Materializes the committed `single-rust` fixture into a throwaway git tree
//! and drives the real umbrella binary (Rust, Go, and command adapters bundled)
//! through an introspection projection (`modules`) and a PLAN-only cut
//! (`plan build`). It stays read-only — no APPLY — proving the shipping umbrella
//! discovers modules and renders a reviewable plan against a real repo. Fixture
//! materialization and the real git import reuse `toven-testkit` primitives
//! rather than an out-of-band shell harness.

use std::path::Path;
use std::process::Command;

use toven_testkit::SampleRepo;

/// Path to the freshly-built umbrella `toven` app binary under test.
const fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_toven")
}

/// Run `toven <args>` in `cwd`, asserting a zero exit and returning stdout.
fn run(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new(binary())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn the toven binary");
    assert!(
        output.status.success(),
        "`toven {}` exited with {:?}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("toven stdout is valid UTF-8")
}

#[test]
fn umbrella_app_discovers_and_plans_a_real_fixture() {
    let repo = SampleRepo::materialize("single-rust").expect("materialize single-rust fixture");
    repo.init_git().expect("init the fixture git tree");
    let root = repo.root();

    let modules = run(root, &["modules"]);
    assert!(
        modules.contains("rust:app"),
        "`modules` should list the rust:app module, got:\n{modules}",
    );

    // Read-only PLAN cut: the umbrella renders a reviewable plan without APPLY.
    run(root, &["plan", "build"]);
}
