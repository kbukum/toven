//! Behavior of the scenario schema loader: well-formed decode, every
//! malformation a typed error, and the Toven default normalizer mapping.
//!
//! Scenario documents live as fixtures under `fixtures/scenarios/` — one
//! directory per case — and are loaded through `fixtures::scenario_path`.

use std::path::Path;

use rskit_errors::AppResult;
use rskit_fs::TempDir;
use toven_testkit::fixtures::scenario_path;
use toven_testkit::scenario::{
    Effect, GitTag, MatcherKind, NormalizeScope, Requires, Scenario, default_normalizer,
};

fn load(name: &str) -> AppResult<Scenario> {
    Scenario::load(&scenario_path(name).expect("scenario fixture exists"))
}

#[test]
fn loads_well_formed_scenario() {
    let scenario = load("valid/full").unwrap();

    assert_eq!(scenario.repo, "rust/workspace-linear");
    assert_eq!(scenario.requires, vec![Requires::Cargo]);
    assert_eq!(scenario.env.get("color").map(String::as_str), Some("never"));

    let git = scenario.git.as_ref().unwrap();
    assert_eq!(git.commits.len(), 1);
    assert_eq!(git.commits[0].msg, "import");
    assert_eq!(git.commits[0].touch, vec!["crates/app/src/main.rs"]);
    assert_eq!(
        git.tags,
        vec![GitTag::head("v1.0.0"), GitTag::at("v0.9.0", 0)]
    );

    assert_eq!(scenario.steps.len(), 2);
    let first = &scenario.steps[0];
    assert_eq!(first.id, "01-plan-build");
    assert_eq!(first.argv, vec!["--output", "jsonl", "plan", "build"]);
    assert_eq!(first.exit, 0);
    assert_eq!(first.stdout.as_ref().unwrap().matcher, MatcherKind::Exact);
    assert!(first.stderr.is_none(), "omitted stream is not asserted");

    let second = &scenario.steps[1];
    assert_eq!(second.config.as_deref(), Some("toven.warm.toml"));
    assert_eq!(second.effects.len(), 2);
    assert!(matches!(second.effects[0], Effect::CacheEntries(_)));
    assert!(matches!(&second.effects[1], Effect::FileExists(path) if path == "target"));
}

#[test]
fn missing_scenario_file_is_not_found() {
    let dir = TempDir::new().unwrap();
    let err = Scenario::load(dir.path()).unwrap_err();
    assert!(err.is_not_found(), "missing scenario.yaml: {err}");
}

#[test]
fn rejects_unknown_keys_at_any_level() {
    for fixture in ["invalid/unknown-top-level-key", "invalid/unknown-step-key"] {
        let err = load(fixture).unwrap_err();
        assert!(
            err.to_string().contains("scenario"),
            "{fixture} must be actionable: {err}"
        );
    }
}

#[test]
fn rejects_duplicate_step_ids() {
    let err = load("invalid/duplicate-step-ids").unwrap_err();
    assert!(
        err.to_string().contains("duplicate step id 'a'"),
        "names the duplicate id: {err}"
    );
}

#[test]
fn rejects_unsafe_step_ids() {
    for fixture in [
        "invalid/step-id-traversal",
        "invalid/step-id-slash",
        "invalid/step-id-empty",
        "invalid/step-id-space",
    ] {
        let err = load(fixture).unwrap_err();
        assert!(
            err.to_string().contains("id"),
            "{fixture} must be rejected: {err}"
        );
    }
}

#[test]
fn rejects_traversing_or_non_toml_config() {
    for fixture in [
        "invalid/config-traversal",
        "invalid/config-subdir",
        "invalid/config-non-toml",
    ] {
        let err = load(fixture).unwrap_err();
        assert!(
            err.to_string().contains("config"),
            "{fixture} must be rejected: {err}"
        );
    }
}

#[test]
fn rejects_unknown_matcher_tier() {
    let err = load("invalid/unknown-matcher").unwrap_err();
    assert!(err.to_string().contains("scenario"), "actionable: {err}");
}

#[test]
fn rejects_unknown_toolchain() {
    let err = load("invalid/unknown-toolchain").unwrap_err();
    assert!(err.to_string().contains("scenario"), "actionable: {err}");
}

#[test]
fn rejects_empty_steps_and_empty_argv() {
    let err = load("invalid/empty-steps").unwrap_err();
    assert!(err.to_string().contains("step"), "empty steps: {err}");

    let err = load("invalid/empty-argv").unwrap_err();
    assert!(err.to_string().contains("argv"), "empty argv: {err}");
}

#[test]
fn rejects_frame_fields_outside_line_set() {
    let err = load("invalid/frame-outside-line-set").unwrap_err();
    assert!(err.to_string().contains("frame"), "actionable: {err}");
}

#[test]
fn cache_entries_comparison_parses_and_matches() {
    let scenario = load("valid/cache-entries").unwrap();

    let Effect::CacheEntries(more_than_zero) = &scenario.steps[0].effects[0] else {
        panic!("expected cache_entries effect");
    };
    assert!(more_than_zero.matches(1));
    assert!(!more_than_zero.matches(0));

    let Effect::CacheEntries(exactly_two) = &scenario.steps[0].effects[1] else {
        panic!("expected cache_entries effect");
    };
    assert!(exactly_two.matches(2));
    assert!(!exactly_two.matches(3));
}

#[test]
fn rejects_unknown_effect_key_and_traversing_effect_golden() {
    // Unknown keys inside an effect map are rejected like any other level.
    let err = load("invalid/effect-unknown-key").unwrap_err();
    assert!(err.to_string().contains("scenario"), "actionable: {err}");

    // The effect golden is a bless-mode write target: traversal is rejected
    // at load time, before anything can run.
    let err = load("invalid/effect-golden-traversal").unwrap_err();
    assert!(err.to_string().contains("golden"), "names the field: {err}");
}

#[test]
fn rejects_malformed_cache_entries_comparison() {
    let err = load("invalid/bad-cache-entries").unwrap_err();
    assert!(err.to_string().contains("scenario"), "actionable: {err}");
}

#[test]
fn default_normalizer_scrubs_toven_volatile_tokens() {
    let scope = NormalizeScope {
        repo_root: Path::new("/tmp/scn-1/repo").into(),
        cache_dir: Path::new("/tmp/scn-1/cache").into(),
    };
    let normalizer = default_normalizer(&scope).unwrap();

    let raw = "built /tmp/scn-1/repo/src in 1.23s; cached /tmp/scn-1/cache/objects \
               at 0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        normalizer.apply(raw),
        "built <REPO>/src in <DUR>; cached <CACHE>/objects at <SHA>"
    );
}

#[test]
fn default_normalizer_scrubs_the_human_summary_duration_line() {
    // The human APPLY summary renders elapsed time as a bare-integer
    // `duration-ms:  N` line (no `ms` suffix), so the `<DUR>` millisecond rule
    // alone leaves it volatile; the dedicated line rule keeps APPLY goldens
    // stable across runs.
    let scope = NormalizeScope {
        repo_root: Path::new("/tmp/scn-1/repo").into(),
        cache_dir: Path::new("/tmp/scn-1/cache").into(),
    };
    let normalizer = default_normalizer(&scope).unwrap();

    assert_eq!(
        normalizer.apply("  duration-ms:  9\n       status:  ok"),
        "  duration-ms:  <DUR>\n       status:  ok"
    );
}

#[test]
fn line_set_expectation_maps_with_frames() {
    let scenario = load("valid/line-set-frames").unwrap();

    let expectation = scenario.steps[0].stdout.as_ref().unwrap();
    assert_eq!(expectation.matcher, MatcherKind::LineSet);

    let scope = NormalizeScope {
        repo_root: Path::new("/r").into(),
        cache_dir: Path::new("/c").into(),
    };
    let matcher = expectation.to_match(&scope).unwrap();
    matcher
        .verify("PLAN\nb\na\nOK\n", "PLAN\na\nb\nOK\n")
        .expect("reordered middle band passes under line-set");
}
