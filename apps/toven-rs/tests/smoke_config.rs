//! Standalone `toven-rs` `--config` variant smokes: one repo tree exercised
//! through several sibling config files (Option A). Each variant lives at the
//! `workspace-linear` repo root beside the happy-path `toven.toml` and is
//! selected explicitly via `--config <file>`, proving config behavior varies
//! without duplicating the manifest tree.

mod common;

use common::{repo, toven_rs_ok};

#[test]
fn unordered_variant_collapses_waves_into_one() {
    let sample = repo("rust/workspace-linear");
    // The graph edges are still discovered, but the run collapses to one wave.
    toven_rs_ok(&sample, &["--config", "toven.unordered.toml", "graph"])
        .expect_stdout_contains("  -> rust:corelib");
    toven_rs_ok(
        &sample,
        &["--config", "toven.unordered.toml", "plan", "build"],
    )
    .expect_stderr_contains("in 1 wave");
}

#[test]
fn json_report_variant_defaults_the_event_sink_to_jsonl() {
    let sample = repo("rust/workspace-linear");
    // No explicit `--output`: the `[toven].report = "json"` default drives it.
    let out = toven_rs_ok(
        &sample,
        &["--config", "toven.json-report.toml", "plan", "build"],
    );
    let first = out.stdout.lines().next().expect("a jsonl line on stdout");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(first).expect("parses")["event"],
        "run-started"
    );
}

#[test]
fn custom_cache_dir_variant_reroots_the_cache_in_the_repo() {
    let sample = repo("rust/workspace-linear");
    let out = toven_rs_ok(
        &sample,
        &["--config", "toven.custom-cache-dir.toml", "cache", "path"],
    );
    assert!(
        out.stdout.trim().contains(".toven/cache"),
        "custom cache dir should reroot under `.toven/cache`, got {:?}",
        out.stdout.trim()
    );
}
