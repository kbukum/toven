//! Standalone `toven-rs` provisioning + generate smokes. The Rust-only binary
//! links just the Rust adapter, so `driver list` reports `rust` linked and every
//! other ecosystem absent. `generate` scaffolds a config for the Rust workspace.

mod common;

use common::{repo, toven_rs, toven_rs_ok};

#[test]
fn driver_list_shows_only_rust_linked() {
    let sample = repo("rust/single");
    toven_rs_ok(&sample, &["driver", "list"])
        .expect_stderr_contains("driver: rust -> linked")
        .expect_stderr_contains("driver: go -> absent")
        .expect_stderr_contains("driver: command -> absent");
}

#[test]
fn generate_stdout_renders_without_writing() {
    let sample = repo("rust/generate-target");
    toven_rs_ok(&sample, &["generate", "--stdout"])
        .expect_stdout_contains("[project]")
        .expect_stdout_contains("[ecosystems.rust]");
    assert!(
        !sample.child("toven.toml").exists(),
        "`generate --stdout` must not write a file"
    );
}

#[test]
fn generate_write_scaffolds_then_is_idempotent() {
    let sample = repo("rust/generate-target");
    toven_rs_ok(&sample, &["generate", "--write"]).expect_stderr_contains("wrote");
    assert!(sample.child("toven.toml").exists());
    toven_rs_ok(&sample, &["generate", "--write"])
        .expect_stderr_contains("already exists; skipping");
}

#[test]
fn generate_stdout_and_write_are_mutually_exclusive() {
    let sample = repo("rust/generate-target");
    assert!(
        !toven_rs(&sample, &["generate", "--stdout", "--write"]).success(),
        "`--stdout --write` must be a usage error"
    );
}
