//! Shared harness for the standalone `toven-go` app smoke suite.
//!
//! Included via `mod common;` by each repo-driven `tests/smoke_*.rs`. Unlike
//! the `__serve` federation smoke (which deliberately avoids the `go`
//! toolchain), the repo-driven flows discover and apply real Go modules, so
//! every one is gated on a `go` toolchain being present via [`go_available`].
#![allow(dead_code)]
// A `tests/common` helper module is private, so `pub(crate)` (needed for parent visibility) reads
// as redundant to clippy's nursery lint; the alternative `pub` trips rustc's `unreachable_pub`.
// Silence the nursery lint here.
#![allow(clippy::redundant_pub_crate)]

use std::path::PathBuf;

use toven_testkit::{RunResult, SampleRepo, program_on_path, run, run_ok};

/// Path to the freshly-built standalone `toven-go` app binary under test.
pub(crate) fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_toven-go"))
}

/// Whether a `go` toolchain is discoverable on `PATH`. Go discovery shells out
/// to `go`, so repo-driven smokes skip (stay green) when it is absent.
pub(crate) fn go_available() -> bool {
    program_on_path("go")
}

/// Materialize a `fixtures/repos/<name>` tree into a throwaway git working
/// tree.
pub(crate) fn repo(name: &str) -> SampleRepo {
    let repo = SampleRepo::materialize(name).expect("materialize fixture repo");
    repo.init_git().expect("init the fixture git tree");
    repo
}

/// Run `toven-go <args>` in the materialized `repo`, capturing both streams.
pub(crate) fn toven_go(repo: &SampleRepo, args: &[&str]) -> RunResult {
    run(&binary(), repo.root(), args)
}

/// Run `toven-go <args>` in the materialized `repo`, asserting a zero exit.
pub(crate) fn toven_go_ok(repo: &SampleRepo, args: &[&str]) -> RunResult {
    run_ok(&binary(), repo.root(), args)
}
