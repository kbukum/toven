//! Standalone `toven-rs` cache-maintenance smokes: `cache path`/`stats`/`clean`
//! plus the `--no-cache` lifecycle. `path`/`stats` render to **stdout**;
//! `clean` diagnostics render to **stderr**.

mod common;

use common::{repo, toven_rs_ok};

/// Parse the `entries:  N` line out of a `cache stats` stdout block.
fn stats_entries(stdout: &str) -> u64 {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("entries:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("no `entries:` line in cache stats:\n{stdout}"))
}

#[test]
fn cache_path_prints_an_absolute_path() {
    let sample = repo("rust/multi-module");
    let out = toven_rs_ok(&sample, &["cache", "path"]);
    assert!(
        std::path::Path::new(out.stdout.trim()).is_absolute(),
        "`cache path` should be absolute, got {:?}",
        out.stdout.trim()
    );
}

#[test]
fn cache_stats_grow_after_apply_and_reset_after_clean() {
    let sample = repo("rust/multi-module");
    toven_rs_ok(&sample, &["cache", "clean"]);
    assert_eq!(
        stats_entries(&toven_rs_ok(&sample, &["cache", "stats"]).stdout),
        0
    );

    toven_rs_ok(&sample, &["check"]);
    assert!(stats_entries(&toven_rs_ok(&sample, &["cache", "stats"]).stdout) > 0);

    toven_rs_ok(&sample, &["cache", "clean"]);
    assert_eq!(
        stats_entries(&toven_rs_ok(&sample, &["cache", "stats"]).stdout),
        0
    );
}

#[test]
fn no_cache_apply_neither_reads_nor_writes() {
    let sample = repo("rust/multi-module");
    toven_rs_ok(&sample, &["cache", "clean"]);
    toven_rs_ok(&sample, &["--no-cache", "check"]);
    assert_eq!(
        stats_entries(&toven_rs_ok(&sample, &["cache", "stats"]).stdout),
        0,
        "`--no-cache` must not write records"
    );
    toven_rs_ok(&sample, &["check"]);
    assert!(stats_entries(&toven_rs_ok(&sample, &["cache", "stats"]).stdout) > 0);
}
