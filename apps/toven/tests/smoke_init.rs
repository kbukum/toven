//! Umbrella `toven` `init` smokes: `--print` preview and the default write +
//! idempotent re-run. The rendered document goes to **stdout**; write
//! diagnostics go to **stderr**. Driven against the `init-target` fixture, which
//! ships a rust workspace deliberately *without* a `toven.toml`. Piped stdio
//! resolves the wizard non-interactively, taking each question's default.

mod common;

use common::{repo, toven_ok};

#[test]
fn init_print_renders_a_document_without_writing() {
    let sample = repo("rust/init-target");
    let out = toven_ok(&sample, &["init", "--print"]);
    out.expect_stdout_contains("[project]")
        .expect_stdout_contains("[ecosystems.rust]");
    assert!(
        !sample.child("toven.toml").exists(),
        "`init --print` must not write a config file"
    );
}

#[test]
fn init_writes_then_is_idempotent() {
    let sample = repo("rust/init-target");

    toven_ok(&sample, &["init"]).expect_stderr_contains("wrote");
    assert!(
        sample.child("toven.toml").exists(),
        "`init` should create toven.toml"
    );

    // A second run adds nothing and reports the existing sections.
    toven_ok(&sample, &["init"]).expect_stderr_contains("already exists; skipping");
}
