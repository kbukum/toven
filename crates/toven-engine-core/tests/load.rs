//! Happy-path loading: every reserved section parses and structurally
//! validates.

mod common;

use common::{canonical, loaded};
use toven_engine_core::config::{ReportFormat, load};
use toven_testkit::{assert_ok, document_path};

fn load_fixture(rel: &str, loaded_ids: &[&str]) -> toven_engine_core::config::Document {
    let path = assert_ok(document_path(rel));
    assert_ok(load(&path, &loaded(loaded_ids), &canonical())).document
}

#[test]
fn loads_minimal_single_ecosystem() {
    let document = load_fixture("valid/single-rust.toml", &["rust"]);

    assert_eq!(document.project.name, "single-rust");
    assert_eq!(document.project.root(), ".");
    assert!(document.ecosystems.contains_key(&common::eid("rust")));
}

#[test]
fn loads_full_polyglot_document() {
    let document = load_fixture("valid/polyglot.toml", &["rust", "go"]);

    assert_eq!(document.project.name, "acme-monorepo");
    assert_eq!(document.project.base_ref.as_deref(), Some("origin/main"));
    assert_eq!(document.toven.report, ReportFormat::Json);
    assert_eq!(document.toven.max_parallel, Some(8));
    assert_eq!(document.toven.cache.dir.as_deref(), Some(".toven/cache"));

    // Reserved sections are typed; ecosystem subtrees are kept verbatim.
    assert!(document.groups.contains_key("core"));
    assert_eq!(
        document.groups["core"].guardrails.forbid,
        ["rust:internal-only"]
    );
    assert_eq!(document.overlays.len(), 1);
    assert_eq!(document.overlays[0].from.ecosystem.as_str(), "go");
    assert_eq!(document.ecosystems.len(), 2);
}

#[test]
fn keeps_ecosystem_subtree_verbatim() {
    let document = load_fixture("valid/polyglot.toml", &["rust", "go"]);

    // The engine never typed-parses the adapter subtree; it stays raw for
    // `Provider::configure`.
    let rust = &document.ecosystems[&common::eid("rust")];
    let manifests = rust
        .get("manifests")
        .and_then(|value| value.as_array())
        .expect("raw manifests array retained");
    assert_eq!(manifests.len(), 2);
}

#[test]
fn loads_multi_repo_members() {
    let document = load_fixture("valid/members.toml", &[]);

    assert_eq!(document.members.len(), 2);
    assert_eq!(document.members[0].name, "core");
    assert_eq!(document.members[0].root, "repos/core");
    assert_eq!(document.members[0].base_ref.as_deref(), Some("origin/main"));
    assert_eq!(document.members[1].base_ref, None);
}

#[test]
fn dotted_key_and_table_forms_load_to_the_same_document() {
    // TOML lets the same data be written with dotted keys (`toven.cache.dir = …`,
    // `ecosystems.rust.release.registry = …`) or with `[table]` headers. The strict
    // loader must treat them as identical, so the two fixtures — one of each form,
    // same data — parse to equal `Document`s.
    let dotted = load_fixture("valid/dotted-keys.toml", &["rust"]);
    let table = load_fixture("valid/table-form.toml", &["rust"]);

    assert_eq!(dotted, table);
}

#[test]
fn merges_include_files_beneath_canonical() {
    // `with-include.toml` declares no groups itself; the included file does.
    let document = load_fixture("valid/with-include.toml", &["rust"]);

    assert!(document.groups.contains_key("core"));
    assert_eq!(document.groups["core"].modules, ["errors", "config"]);
}

#[test]
fn well_formed_but_unresolved_ref_loads() {
    // A group ref to a module that may not exist is STRUCTURALLY valid; semantic
    // resolution is deferred to the Graph phase, so the document loads.
    let document = load_fixture("valid/nonexistent-module-ref.toml", &["rust"]);

    assert_eq!(document.groups["core"].modules, ["rust:does-not-exist"]);
}

#[test]
fn distinct_overlays_from_includes_concatenate() {
    // The canonical file declares one overlay edge; the include declares another.
    // Distinct overlay identities concatenate across files (not replaced), so both
    // edges survive the merge.
    let document = load_fixture("valid/overlay-concat-include.toml", &["rust", "go"]);

    assert_eq!(document.overlays.len(), 2);
    let modules: Vec<&str> = document
        .overlays
        .iter()
        .map(|overlay| overlay.from.module.as_str())
        .collect();
    assert!(modules.contains(&"api"));
    assert!(modules.contains(&"worker"));
}

#[test]
fn loads_per_module_release_override() {
    // `[modules."rust:core".release]` parses into the typed override and keeps its
    // set fields; the ecosystem `[ecosystems.rust.release]` default parses too.
    let document = load_fixture("valid/release-overrides.toml", &["rust"]);

    let over = &document.modules["rust:core"].release;
    assert_eq!(
        over.level,
        Some(toven_ports::BumpLevel::Major),
        "per-module level override is retained"
    );
    assert_eq!(over.tag_format.as_deref(), Some("core-v{version}"));
    assert!(
        over.registry.is_none(),
        "unset override fields stay None so the ecosystem default shows through"
    );
}

#[test]
fn loads_project_level_verb_hooks() {
    use toven_engine_core::config::VerbId;

    let document = load_fixture("valid/verb-hooks.toml", &["rust"]);

    assert_eq!(document.hooks[&VerbId::Release].pre, ["fmt-check", "lint"]);
    assert_eq!(document.hooks[&VerbId::Release].post, ["notify-release"]);
    assert_eq!(document.hooks[&VerbId::Bump].pre, ["validate", "lint"]);
    assert_eq!(document.hooks[&VerbId::Coverage].pre, ["build"]);

    // A plain verb resolves to only its own hooks.
    let coverage = document.hooks_for(VerbId::Coverage);
    assert_eq!(coverage.pre, ["build"]);
    assert!(coverage.post.is_empty());

    // A release mutation composes the umbrella around its own hooks: the
    // umbrella's `pre` runs first (specific innermost), and its `post` runs last.
    // Composition is a concatenation, not a set union: `lint` is authored in
    // both the umbrella and the specific verb, so it is deliberately kept and
    // runs once per occurrence rather than being de-duplicated.
    let bump = document.hooks_for(VerbId::Bump);
    assert_eq!(bump.pre, ["fmt-check", "lint", "validate", "lint"]);
    assert_eq!(bump.post, ["notify-release"]);

    // A verb with no configured hooks and no umbrella resolves empty.
    assert!(document.hooks_for(VerbId::Doctor).is_empty());
}
