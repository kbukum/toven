//! Guard that the documented config snippets stay loadable.
//!
//! The `toml` fenced blocks are **extracted from the doc files at test time**
//! (`README.md`, `docs/getting-started.md`) and loaded through the strict
//! `Document` loader, so an edit to a documented snippet is validated against the
//! live schema without a copy in this test drifting out of sync. A block that is
//! a bare fragment (no `[project]` header) is loaded under a minimal synthesized
//! project so a documented task/ecosystem fragment is still schema-checked.

mod common;

use std::path::{Path, PathBuf};

use common::{canonical, loaded};
use rskit_fs::TempDir;
use rskit_fs::sync_io::file::{read_string_bounded, write};
use toven_engine::config::load;
use toven_testkit::assert_ok;

/// Upper bound on a doc file read (generous; these are small Markdown files).
const MAX_DOC_BYTES: u64 = 4 * 1024 * 1024;

/// A minimal, schema-valid project header prepended to a documented fragment
/// (a block without its own `[project]`) so the fragment can be loaded.
const FRAGMENT_HEADER: &str = "[project]\nname = \"demo\"\nroot = \".\"\n";

/// Resolve a workspace-root-relative doc path from this crate's manifest dir.
fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// Extract every fenced `toml` code-block body from `markdown`.
fn toml_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in markdown.lines() {
        match &mut current {
            None if line.trim_start().starts_with("```toml") => current = Some(Vec::new()),
            None => {}
            Some(body) if line.trim_start().starts_with("```") => {
                blocks.push(body.join("\n"));
                current = None;
            }
            Some(body) => body.push(line),
        }
    }
    blocks
}

/// Load a documented snippet through the strict loader from a temp file,
/// synthesizing a `[project]` header when the block is a bare fragment.
fn load_block(block: &str) {
    let toml = if block.contains("[project]") {
        block.to_string()
    } else {
        format!("{FRAGMENT_HEADER}\n{block}")
    };
    let dir = assert_ok(TempDir::new());
    let path = dir.path().join("toven.toml");
    assert_ok(write(&path, toml.as_bytes()));
    assert_ok(load(&path, &loaded(&["rust"]), &canonical()));
}

/// Every `toml` block in `doc` (relative to the workspace root) loads through the
/// strict loader; the file must contain at least one block so a doc restructure
/// that drops the examples is caught rather than silently passing.
fn assert_doc_snippets_load(doc: &str) {
    let markdown = assert_ok(read_string_bounded(&workspace_path(doc), MAX_DOC_BYTES));
    let blocks = toml_blocks(&markdown);
    assert!(!blocks.is_empty(), "no `toml` blocks found in {doc}");
    for block in blocks {
        load_block(&block);
    }
}

#[test]
fn readme_config_snippets_round_trip() {
    assert_doc_snippets_load("README.md");
}

#[test]
fn getting_started_config_snippets_round_trip() {
    assert_doc_snippets_load("docs/getting-started.md");
}
