//! Ecosystem-id three-way dispatch (loaded / canonical-unloaded / unknown).

mod common;

use common::{canonical, eid, loaded};
use rskit_errors::ErrorCode;
use toven_engine::config::load;
use toven_testkit::{assert_err_code, assert_ok, document_path};

#[test]
fn loaded_ecosystem_is_configurable() {
    let path = assert_ok(document_path("valid/single-rust.toml"));
    let loaded = assert_ok(load(&path, &loaded(&["rust"]), &canonical()));

    let result = loaded.dispatch;

    assert!(result.configurable.contains_key(&eid("rust")));
    assert!(result.ignored.is_empty());
}

#[test]
fn canonical_but_unloaded_ecosystem_is_warned_and_ignored() {
    let path = assert_ok(document_path("valid/canonical-unloaded.toml"));
    // Only Rust is loaded; `[ecosystems.go]` is canonical but unloaded here.
    let loaded = assert_ok(load(&path, &loaded(&["rust"]), &canonical()));

    let result = loaded.dispatch;

    assert!(result.configurable.contains_key(&eid("rust")));
    assert_eq!(result.ignored, vec![eid("go")]);
}

#[test]
fn unknown_ecosystem_id_is_a_hard_error() {
    let path = assert_ok(document_path("invalid/unknown-ecosystem.toml"));

    // `rsut` is neither loaded nor canonical: loading fails outright.
    assert_err_code(
        load(&path, &loaded(&["rust"]), &canonical()),
        ErrorCode::InvalidInput,
    );
}
