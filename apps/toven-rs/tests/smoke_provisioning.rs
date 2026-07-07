//! Standalone `toven-rs` provisioning + init smokes. The Rust-only binary links
//! just the Rust adapter, so `driver list` reports `rust` linked and every other
//! ecosystem absent. `init` onboards a config for the Rust workspace.

mod common;

use common::{repo, toven_rs_ok};

#[test]
fn driver_list_shows_only_rust_linked() {
    let sample = repo("rust/single");
    toven_rs_ok(&sample, &["driver", "list"])
        .expect_stderr_contains("driver: rust -> linked")
        .expect_stderr_contains("driver: go -> absent")
        .expect_stderr_contains("driver: command -> absent");
}

#[test]
fn init_print_renders_without_writing() {
    let sample = repo("rust/init-target");
    toven_rs_ok(&sample, &["init", "--print"])
        .expect_stdout_contains("[project]")
        .expect_stdout_contains("[ecosystems.rust]");
    assert!(
        !sample.child("toven.toml").exists(),
        "`init --print` must not write a file"
    );
}

#[test]
fn init_writes_then_is_idempotent() {
    let sample = repo("rust/init-target");
    toven_rs_ok(&sample, &["init"]).expect_stderr_contains("wrote");
    assert!(sample.child("toven.toml").exists());
    toven_rs_ok(&sample, &["init"]).expect_stderr_contains("already exists; skipping");
}
