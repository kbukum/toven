//! `toven doctor` scope for the Go adapter, driven by real `go` against
//! fixture workspaces.

use std::sync::Arc;

use toven_exec::ProcessToolRunner;
use toven_go::GoProvider;
use toven_model::AbsPath;
use toven_ports::{CheckVerdict, ConfiguredAdapter, Provider};

fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../toven-testkit/fixtures/ecosystems/go")
        .join(rel)
}

fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    let text = std::fs::read_to_string(fixture(adapter_config)).expect("fixture");
    let raw = rskit_codec::decode(&rskit_codec::TomlCodec, &text).expect("raw subtree");
    GoProvider::new(Arc::new(ProcessToolRunner::new()))
        .expect("provider")
        .configure(raw)
        .expect("configure")
}

/// `(id, dir)` for every toolchain check the adapter returns.
fn toolchain_checks(adapter_config: &str, workspace: &str) -> Vec<(String, String)> {
    let root = AbsPath::new(fixture(workspace)).expect("absolute root");
    let scope = configure(adapter_config).doctor(&root).expect("doctor");
    assert_eq!(scope.toolchains.len(), 1);
    assert_eq!(scope.toolchains[0].tool, "go");
    scope
        .checks
        .into_iter()
        .map(|check| {
            assert!(check.fix.is_some(), "{} has a fix", check.id);
            match check.verdict {
                CheckVerdict::Command { argv, dir, version } => {
                    assert_eq!(argv, ["go", "list", "-m"]);
                    assert_eq!(version, None);
                    (check.id, dir.to_string())
                }
                other => panic!("unexpected verdict {other:?}"),
            }
        })
        .collect()
}

#[test]
fn a_go_work_is_checked_once_at_its_root() {
    assert_eq!(
        toolchain_checks("adapter/auto-modules.toml", "workspaces/work"),
        [("toolchain".to_string(), ".".to_string())]
    );
}

#[test]
fn modules_outside_a_workspace_are_checked_in_their_own_directory() {
    assert_eq!(
        toolchain_checks("adapter/work-modules.toml", "workspaces/replacements"),
        [
            ("app/toolchain".to_string(), "app".to_string()),
            ("core/toolchain".to_string(), "core".to_string()),
        ]
    );
}

#[test]
fn a_root_module_is_checked_at_the_repository_root() {
    assert_eq!(
        toolchain_checks("adapter/auto-modules.toml", "workspaces/single-module"),
        [("toolchain".to_string(), ".".to_string())]
    );
}
