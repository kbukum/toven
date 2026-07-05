//! Umbrella `toven` cache-maintenance smokes: `cache path`/`stats`/`clean` plus
//! the `--no-cache` lifecycle. `cache path`/`stats` render to **stdout**;
//! `cache clean` diagnostics render to **stderr**.

mod common;

use common::{repo, toven_ok};

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
    let out = toven_ok(&sample, &["cache", "path"]);
    let path = out.stdout.trim();
    assert!(
        std::path::Path::new(path).is_absolute(),
        "`cache path` should print an absolute path, got {path:?}"
    );
}

#[test]
fn cache_stats_grow_after_an_apply_and_reset_after_clean() {
    let sample = repo("rust/multi-module");

    // Start from a clean slate, then apply a cache-writing task.
    toven_ok(&sample, &["cache", "clean"]);
    assert_eq!(
        stats_entries(&toven_ok(&sample, &["cache", "stats"]).stdout),
        0
    );

    toven_ok(&sample, &["check"]);
    assert!(
        stats_entries(&toven_ok(&sample, &["cache", "stats"]).stdout) > 0,
        "an APPLY should populate the cache"
    );

    // `clean` empties the store again.
    toven_ok(&sample, &["cache", "clean"]);
    assert_eq!(
        stats_entries(&toven_ok(&sample, &["cache", "stats"]).stdout),
        0
    );
}

#[test]
fn no_cache_apply_neither_reads_nor_writes_the_cache() {
    let sample = repo("rust/multi-module");
    toven_ok(&sample, &["cache", "clean"]);

    // A `--no-cache` run applies but records nothing.
    toven_ok(&sample, &["--no-cache", "check"]);
    assert_eq!(
        stats_entries(&toven_ok(&sample, &["cache", "stats"]).stdout),
        0,
        "`--no-cache` must not write cache records"
    );

    // A normal run does record, proving the previous run's suppression was real.
    toven_ok(&sample, &["check"]);
    assert!(stats_entries(&toven_ok(&sample, &["cache", "stats"]).stdout) > 0);
}
