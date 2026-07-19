//! Structural validation: malformed documents fail at load time.

mod common;

use common::{canonical, loaded};
use rskit_errors::ErrorCode;
use toven_engine::config::load;
use toven_testkit::{assert_err_code, assert_ok, document_path};

fn assert_rejected(rel: &str, loaded_ids: &[&str]) {
    let path = assert_ok(document_path(rel));
    assert_err_code(
        load(&path, &loaded(loaded_ids), &canonical()),
        ErrorCode::InvalidInput,
    );
}

#[test]
fn unknown_top_level_key_is_rejected() {
    assert_rejected("invalid/unknown-top-level-key.toml", &["rust"]);
}

#[test]
fn duplicate_group_identity_is_rejected() {
    // Single-file duplicate group: TOML itself rejects the redefined
    // `[groups.core]` table header during decode.
    assert_rejected("invalid/duplicate-group.toml", &["rust"]);
}

#[test]
fn duplicate_group_identity_across_includes_is_rejected() {
    // Cross-file duplicate group: the canonical file and an include both declare
    // `[groups.core]`. The include-merge policy registers `groups` as unique-keyed,
    // so the collision is a hard error instead of a silent recursive merge.
    assert_rejected("invalid/duplicate-group-include.toml", &["rust"]);
}

#[test]
fn duplicate_overlay_identity_across_includes_is_rejected() {
    // Cross-file duplicate overlay: both files declare the same `from`/`to` edge,
    // caught by the overlay composite identity.
    assert_rejected("invalid/duplicate-overlay-include.toml", &["rust", "go"]);
}

#[test]
fn duplicate_member_identity_across_includes_is_rejected() {
    // Cross-file duplicate member: the canonical file and an include both declare a
    // `[[members]]` entry named `core`. The include-merge policy registers
    // `members` with a `name` identity, so the collision is a hard error instead of
    // a silently concatenated entry.
    assert_rejected("invalid/duplicate-member-include.toml", &["rust"]);
}

#[test]
fn unsafe_cache_dir_is_rejected() {
    // `[toven.cache].dir` is a workspace-relative path used later for filesystem
    // writes, so a traversal escape must fail at the trust boundary.
    assert_rejected("invalid/unsafe-cache-dir.toml", &["rust"]);
}

#[test]
fn malformed_overlay_ref_is_rejected() {
    assert_rejected("invalid/malformed-overlay.toml", &["rust", "go"]);
}

#[test]
fn malformed_group_ref_is_rejected() {
    assert_rejected("invalid/malformed-group-ref.toml", &["rust"]);
}

#[test]
fn group_name_with_reserved_separator_is_rejected() {
    // A `~` in a group name would shadow the scheduler's `~~L{layer}` unit-id
    // marker, so it is rejected at the config boundary.
    assert_rejected("invalid/group-name-reserved-separator.toml", &["rust"]);
}

#[test]
fn per_module_release_unknown_field_is_rejected() {
    // `[modules.<name>.release]` is strict: an unknown key fails the decode.
    assert_rejected("invalid/release-module-unknown-field.toml", &["rust"]);
}

#[test]
fn per_module_release_bad_tag_template_is_rejected() {
    // An unknown tag-template placeholder in a per-module override fails structural
    // validation.
    assert_rejected("invalid/release-module-bad-tag-template.toml", &["rust"]);
}

#[test]
fn per_module_coverage_out_of_range_is_rejected() {
    // A per-module coverage floor outside `0.0..=100.0` fails structural
    // validation.
    assert_rejected("invalid/coverage-module-out-of-range.toml", &["rust"]);
}

#[test]
fn per_module_coverage_ecosystem_only_field_is_rejected() {
    // `exclude`/`profiles` are ecosystem-level decisions that never affect gating
    // inside a `[modules.<ref>.coverage]` block, so accepting them there is
    // rejected rather than silently ignored.
    assert_rejected(
        "invalid/coverage-module-ecosystem-only-field.toml",
        &["rust"],
    );
}

#[test]
fn per_module_release_unqualified_ref_is_rejected() {
    // A `[modules.<name>]` key must be a qualified `ecosystem:module` reference.
    assert_rejected("invalid/release-module-unqualified-ref.toml", &["rust"]);
}
