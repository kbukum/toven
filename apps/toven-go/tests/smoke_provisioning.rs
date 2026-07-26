//! Standalone `toven-go` provisioning smoke: `driver list`. The Go-only binary
//! links just the Go adapter, so `go` reports linked and every other ecosystem
//! absent. `driver list` performs no module discovery, so it needs no toolchain
//! and stays ungated. Status lines render to **stdout**.

mod common;

use common::{repo, toven_go_ok};

#[test]
fn driver_list_shows_only_go_linked() {
    let sample = repo("go/single");
    toven_go_ok(&sample, &["driver", "list"])
        .expect_stdout_contains("driver: go -> linked")
        .expect_stdout_contains("driver: rust -> absent")
        .expect_stdout_contains("driver: command -> absent");
}
