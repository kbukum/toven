//! Shared harness for the umbrella `toven` app smoke suite.
//!
//! Each `tests/smoke_*.rs` file is its own integration crate; this module is
//! included via `mod common;` so they share one binary resolver + fixture
//! materializer instead of re-declaring process plumbing. The real end-to-end
//! machinery lives in [`toven_testkit::smoke`]; this only binds it to the
//! umbrella binary.
#![allow(dead_code)]
// A `tests/common` helper module is private, so `pub(crate)` (needed for parent visibility) reads
// as redundant to clippy's nursery lint; the alternative `pub` trips rustc's `unreachable_pub`.
// Silence the nursery lint here.
#![allow(clippy::redundant_pub_crate)]

use std::path::PathBuf;

use toven_testkit::{RunResult, SampleRepo, run, run_ok};

/// Path to the freshly-built umbrella `toven` app binary under test.
///
/// `env!("CARGO_BIN_EXE_toven")` only expands inside this app crate, so the
/// shared harness receives the resolved path from here.
pub(crate) fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_toven"))
}

/// Materialize a `fixtures/repos/<name>` tree into a throwaway git working tree
/// the real binary can plan/apply against.
pub(crate) fn repo(name: &str) -> SampleRepo {
    let repo = SampleRepo::materialize(name).expect("materialize fixture repo");
    repo.init_git().expect("init the fixture git tree");
    repo
}

/// Run `toven <args>` in the materialized `repo`, capturing both streams.
pub(crate) fn toven(repo: &SampleRepo, args: &[&str]) -> RunResult {
    run(&binary(), repo.root(), args)
}

/// Run `toven <args>` in the materialized `repo`, asserting a zero exit.
pub(crate) fn toven_ok(repo: &SampleRepo, args: &[&str]) -> RunResult {
    run_ok(&binary(), repo.root(), args)
}
