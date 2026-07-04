//! Umbrella `toven` provisioning/federation smokes: `driver list` and
//! `federation status`. The umbrella bundles the Rust, Go, and command adapters,
//! so all three report as linked. Status lines render to **stderr**.

mod common;

use common::{repo, toven_ok};

#[test]
fn driver_list_shows_every_bundled_adapter_linked() {
    let sample = repo("cross-ecosystem/umbrella");
    let out = toven_ok(&sample, &["driver", "list"]);
    out.expect_stderr_contains("driver: rust -> linked")
        .expect_stderr_contains("driver: go -> linked")
        .expect_stderr_contains("driver: command -> linked");
}

#[test]
fn federation_status_reports_linked_drivers() {
    let sample = repo("federation/cross-repo");
    toven_ok(&sample, &["federation", "status"])
        .expect_stderr_contains("federation: rust -> linked");
}

#[test]
fn cross_repo_discovers_every_member_module() {
    // A `[[members]]` federation composes each member's `toven.toml` into one
    // graph, so both member-scoped modules are discovered under the umbrella.
    let sample = repo("federation/cross-repo");
    toven_ok(&sample, &["modules"])
        .expect_stdout_contains("core/rust:core")
        .expect_stdout_contains("services/rust:services");
}
