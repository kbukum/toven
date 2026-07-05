//! Umbrella `toven` `generate` smokes: `--stdout` preview, `--write` scaffold +
//! idempotent re-write, and the `--stdout`/`--write` mutual-exclusion usage
//! error. The rendered document goes to **stdout**; write diagnostics go to
//! **stderr**. Driven against the `generate-target` fixture, which ships a rust
//! workspace deliberately *without* a `toven.toml`.

mod common;

use common::{repo, toven, toven_ok};

#[test]
fn generate_stdout_renders_a_document_without_writing() {
    let sample = repo("rust/generate-target");
    let out = toven_ok(&sample, &["generate", "--stdout"]);
    out.expect_stdout_contains("[project]")
        .expect_stdout_contains("[ecosystems.rust]");
    assert!(
        !sample.child("toven.toml").exists(),
        "`generate --stdout` must not write a config file"
    );
}

#[test]
fn generate_write_scaffolds_then_is_idempotent() {
    let sample = repo("rust/generate-target");

    toven_ok(&sample, &["generate", "--write"]).expect_stderr_contains("wrote");
    assert!(
        sample.child("toven.toml").exists(),
        "`generate --write` should create toven.toml"
    );

    // A second write adds nothing and reports the existing sections.
    toven_ok(&sample, &["generate", "--write"]).expect_stderr_contains("already exists; skipping");
}

#[test]
fn generate_stdout_and_write_are_mutually_exclusive() {
    let sample = repo("rust/generate-target");
    let out = toven(&sample, &["generate", "--stdout", "--write"]);
    assert!(!out.success(), "`--stdout --write` must be a usage error");
}
