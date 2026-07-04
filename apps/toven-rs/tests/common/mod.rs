//! Shared harness for the standalone `toven-rs` app smoke suite.
//!
//! Each `tests/smoke_*.rs` file is its own integration crate; this module is
//! included via `mod common;` so they share one binary resolver + fixture
//! materializer. The end-to-end machinery lives in [`toven_testkit::smoke`];
//! this binds it to the Rust-only `toven-rs` binary.
#![allow(dead_code)]
// A `tests/common` helper module is private, so `pub(crate)` (needed for parent
// visibility) reads as redundant to clippy's nursery lint; the alternative `pub`
// trips rustc's `unreachable_pub`. Silence the nursery lint here.
#![allow(clippy::redundant_pub_crate)]

use std::path::PathBuf;

use toven_testkit::{RunResult, SampleRepo, run, run_ok};

/// Path to the freshly-built standalone `toven-rs` app binary under test.
pub(crate) fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_toven-rs"))
}

/// Materialize a `fixtures/repos/<name>` tree into a throwaway git working tree.
pub(crate) fn repo(name: &str) -> SampleRepo {
    let repo = SampleRepo::materialize(name).expect("materialize fixture repo");
    repo.init_git().expect("init the fixture git tree");
    repo
}

/// Run `toven-rs <args>` in the materialized `repo`, capturing both streams.
pub(crate) fn toven_rs(repo: &SampleRepo, args: &[&str]) -> RunResult {
    run(&binary(), repo.root(), args)
}

/// Run `toven-rs <args>` in the materialized `repo`, asserting a zero exit.
pub(crate) fn toven_rs_ok(repo: &SampleRepo, args: &[&str]) -> RunResult {
    run_ok(&binary(), repo.root(), args)
}
