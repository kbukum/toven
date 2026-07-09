//! Guard that the documented config snippets stay loadable.
//!
//! The strings below are the minimal Rust config and the task-override example
//! shown verbatim in `README.md` / `docs/getting-started.md`. Loading them
//! through the strict `Document` loader fails the build the moment a doc example
//! drifts from the live schema.

mod common;

use common::{canonical, eid, loaded};
use rskit_fs::TempDir;
use rskit_fs::sync_io::file::write;
use toven_engine::config::load;
use toven_testkit::assert_ok;

/// Load a documented snippet through the strict loader from a temp file.
fn load_snippet(toml: &str) -> toven_engine::config::Document {
    let dir = assert_ok(TempDir::new());
    let path = dir.path().join("toven.toml");
    assert_ok(write(&path, toml.as_bytes()));
    assert_ok(load(&path, &loaded(&["rust"]), &canonical())).document
}

#[test]
fn readme_minimal_rust_config_round_trips() {
    let document = load_snippet(
        r#"
[project]
name = "demo"
root = "."
base_ref = "origin/main"

[ecosystems.rust]
manifests = ["Cargo.toml"]
"#,
    );

    assert_eq!(document.project.name, "demo");
    assert_eq!(document.project.base_ref.as_deref(), Some("origin/main"));
    assert!(document.ecosystems.contains_key(&eid("rust")));
}

#[test]
fn readme_task_override_example_round_trips() {
    let document = load_snippet(
        r#"
[project]
name = "demo"
root = "."

[ecosystems.rust]
manifests = ["Cargo.toml"]

[ecosystems.rust.tasks.test]
argv = ["cargo", "test", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
selector = ["-p", "{module.package}"]
cache_args = true
shared_inputs = ["Cargo.lock", "rust-toolchain.toml"]
"#,
    );

    assert!(document.ecosystems.contains_key(&eid("rust")));
}
