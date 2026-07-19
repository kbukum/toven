//! A [`TestWorkspace`] pointed at the shared `toven-testkit` fixture root.
//!
//! `rskit-testutil`'s [`test_workspace!`](rskit_testutil::test_workspace) macro
//! roots fixtures at the *calling* crate's `tests/fixtures`. Toven instead
//! wants one shared tree, so this module hands back a [`TestWorkspace`] whose
//! fixture root is **this** crate's `fixtures/` directory regardless of the
//! consumer.

use std::path::PathBuf;

use rskit_testutil::TestWorkspace;

use crate::fixtures;

/// The shared fixture root used by [`workspace`].
#[must_use]
pub fn fixtures_root() -> PathBuf {
    fixtures::root()
}

/// Create a managed [`TestWorkspace`] rooted at the shared fixture tree.
///
/// The workspace is a temp dir deleted on drop; `copy_fixture`/`read_fixture`
/// resolve against `toven-testkit/fixtures` (not the consumer crate).
#[must_use]
pub fn workspace(label: &str) -> TestWorkspace {
    TestWorkspace::new(label).with_fixture_dir(fixtures_root())
}

#[cfg(test)]
mod tests {
    use super::workspace;

    #[test]
    fn copies_shared_fixture_into_temp_workspace() {
        let ws = workspace("shared");

        let copied = ws
            .copy_fixture("config/valid/single-rust.toml", "toven.toml")
            .expect("copies shared fixture");

        assert!(copied.starts_with(ws.path()));
        let body = rskit_fs::sync_io::file::read_string(&copied).expect("reads copy");
        assert!(body.contains("[project]"));
    }
}
