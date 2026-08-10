//! Go release-flow artifact parity: native packaging and `GoReleaser` delegation.
//!
//! Proves the Go sliver of the language-agnostic release flow end to end over
//! the public [`release_package`] facade:
//!
//! * **native** — Toven archives an already-built Go binary into its declared
//!   per-target `host.assets` archive (deterministic `tar.gz`, and a Windows
//!   `.zip` carrying the `.exe` member), reporting `backing = "native"`;
//! * **delegated** — the Go `Package` phase is backed by `GoReleaser` through the
//!   `ToolRunner` seam: Toven drives a mutation-free `--snapshot` preview,
//!   then normalizes the tool-produced archive back into the typed report as
//!   `backing = "delegated"`; and
//! * **multi-module** — in a Go multi-module fixture the binary module attaches
//!   its archives (native *and* GoReleaser-delegated) while the sibling library
//!   modules, which declare no host assets, contribute none — the lock-step
//!   library tags are a plan/tag concern the packaging phase never touches.
//!
//! Toven still owns which asset maps to which target and that the archive exists
//! in both backings; the tool only produces bytes. Network-free, deterministic.

use std::collections::BTreeMap;
use std::path::Path;

use rskit_config::RawValue;
use rskit_fs::TempDir;
use serde_json::json;
use toven_engine_core::config::{Document, ProjectConfig, TovenConfig};
use toven_engine_core::plan::PlanRequest;
use toven_engine_release::release_package;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, ReleasePhase, RepoPath};
use toven_ports::{
    CommonEcosystemConfig, DelegatedTool, DiscoverResponse, HostConfig, PhaseBackingKind,
    PhaseConfig, PhasesConfig, Provider, ReleaseConfig, TaskIntent,
};
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeToolRunner, RecordingReporter,
};

const LINUX: &str = "x86_64-unknown-linux-gnu";
const WINDOWS: &str = "x86_64-pc-windows-msvc";

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

fn go_module(name: &str, root: &str) -> Module {
    Module::new(
        ModuleRef::new(eid("go"), name).expect("go module ref"),
        RepoPath::new(root).expect("module root"),
    )
}

fn document() -> Document {
    let mut ecosystems = BTreeMap::new();
    ecosystems.insert(eid("go"), RawValue::from(json!({ "release": {} })));
    Document {
        project: ProjectConfig {
            name: "go-demo".to_string(),
            root: ".".to_string(),
            base_ref: None,
        },
        toven: TovenConfig::default(),
        groups: BTreeMap::new(),
        overlays: Vec::new(),
        ecosystems,
        modules: BTreeMap::new(),
        members: Vec::new(),
        hooks: std::collections::BTreeMap::new(),
    }
}

fn request(root: &Path) -> PlanRequest {
    PlanRequest::new(
        "go-rel",
        "go-demo",
        TaskIntent::resolve("release"),
        AbsPath::new(root.to_str().expect("utf-8 root")).expect("absolute root"),
    )
}

/// A delegated `GoReleaser` `Package` backing whose mutation-free preview is a
/// `--snapshot` invocation.
fn goreleaser_package_phase() -> PhasesConfig {
    let mut phases = BTreeMap::new();
    phases.insert(
        ReleasePhase::Package,
        PhaseConfig {
            backing: PhaseBackingKind::Delegated,
            delegated: Some(DelegatedTool {
                tool: "goreleaser".into(),
                args: Some(vec!["release".into(), "--clean".into()]),
                preview: vec!["release".into(), "--snapshot".into(), "--clean".into()],
            }),
        },
    );
    PhasesConfig(phases)
}

/// A Go provider whose binary module declares `assets` on a `github` forge,
/// optionally delegating the `Package` phase to `GoReleaser`. Sibling library
/// modules declare no host assets.
fn go_provider(assets: Vec<&str>, delegated_package: bool) -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("go"));
    response.modules = vec![
        go_module("logging", "internal/logging"),
        go_module("api", "internal/api"),
        go_module("cli", "cmd/cli"),
    ];
    let mut release = ReleaseConfig {
        host: Some(HostConfig {
            forge: Some("github".to_string()),
            assets: Some(assets.into_iter().map(str::to_string).collect()),
            ..HostConfig::default()
        }),
        ..ReleaseConfig::default()
    };
    if delegated_package {
        release.phases = Some(goreleaser_package_phase());
    }
    let common = CommonEcosystemConfig {
        release,
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid("go"))
        .with_response(response)
        .with_release_target(FakeReleaseTarget::new())
        .with_common(common);
    FakeProvider::new(eid("go")).with_adapter(adapter)
}

/// Write a built Go binary at `path` under `root`, returning nothing.
fn write_go_binary(root: &Path, rel: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create binary dir");
    }
    std::fs::write(&path, b"\x7fELF-fake-go-binary").expect("write binary");
}

#[test]
fn native_packages_a_built_go_binary_into_the_declared_tar_gz() {
    let root = TempDir::new().unwrap();
    write_go_binary(root.path(), "bin/cli");
    let binary = root.path().join("bin/cli");
    let provider = go_provider(vec!["dist/cli-x86_64-unknown-linux-gnu.tar.gz"], false);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let mut reporter = RecordingReporter::new();

    let report = release_package(
        &request(root.path()),
        &document(),
        &providers,
        LINUX,
        Some(binary.as_path()),
        &FakeToolRunner::new(),
        &mut reporter,
    )
    .expect("native go package runs");

    assert_eq!(report.assets.len(), 1);
    let asset = &report.assets[0];
    assert_eq!(asset.asset, "dist/cli-x86_64-unknown-linux-gnu.tar.gz");
    assert_eq!(asset.backing, "native");
    assert!(asset.bytes > 0);
    assert!(
        root.path()
            .join("dist/cli-x86_64-unknown-linux-gnu.tar.gz")
            .is_file(),
        "the archive must be written to the declared asset path"
    );
}

#[test]
fn native_packages_a_built_go_binary_into_a_windows_zip() {
    let root = TempDir::new().unwrap();
    write_go_binary(root.path(), "bin/cli.exe");
    let binary = root.path().join("bin/cli.exe");
    let provider = go_provider(vec!["dist/cli-x86_64-pc-windows-msvc.zip"], false);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let mut reporter = RecordingReporter::new();

    let report = release_package(
        &request(root.path()),
        &document(),
        &providers,
        WINDOWS,
        Some(binary.as_path()),
        &FakeToolRunner::new(),
        &mut reporter,
    )
    .expect("native go windows package runs");

    let asset = &report.assets[0];
    assert_eq!(asset.asset, "dist/cli-x86_64-pc-windows-msvc.zip");
    assert_eq!(asset.backing, "native");
    assert!(
        root.path()
            .join("dist/cli-x86_64-pc-windows-msvc.zip")
            .is_file()
    );
}

#[test]
fn goreleaser_delegated_package_runs_a_snapshot_preview_and_normalizes_the_archive() {
    let root = TempDir::new().unwrap();
    let asset_rel = "dist/cli-x86_64-unknown-linux-gnu.tar.gz";
    // No binary is built natively: GoReleaser "produces" the archive, which the
    // runner writes on a successful mutation-free preview.
    let runner = FakeToolRunner::new()
        .with_produced_file(root.path().join(asset_rel), b"goreleaser-archive");
    let provider = go_provider(vec![asset_rel], true);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let mut reporter = RecordingReporter::new();

    let report = release_package(
        &request(root.path()),
        &document(),
        &providers,
        LINUX,
        None,
        &runner,
        &mut reporter,
    )
    .expect("delegated go package runs");

    let asset = &report.assets[0];
    assert_eq!(asset.asset, asset_rel);
    assert_eq!(asset.backing, "delegated");
    assert!(asset.bytes > 0);
    // GoReleaser was driven argv-first, tool-first, as a mutation-free snapshot.
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].argv.first().map(String::as_str),
        Some("goreleaser")
    );
    assert!(requests[0].argv.iter().any(|arg| arg == "--snapshot"));
}

#[test]
fn goreleaser_delegated_package_fails_closed_when_no_archive_is_produced() {
    let root = TempDir::new().unwrap();
    // The tool exits zero but produces nothing.
    let runner = FakeToolRunner::new();
    let provider = go_provider(vec!["dist/cli-x86_64-unknown-linux-gnu.tar.gz"], true);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let mut reporter = RecordingReporter::new();

    let error = release_package(
        &request(root.path()),
        &document(),
        &providers,
        LINUX,
        None,
        &runner,
        &mut reporter,
    )
    .expect_err("a delegated tool producing no archive must fail closed");
    assert!(error.to_string().contains("did not produce"), "{error}");
}

#[test]
fn only_the_binary_module_attaches_archives_across_backings() {
    // The Go multi-module fixture declares three modules; only the binary `cli`
    // module carries host assets, so both backings archive exactly one asset —
    // the sibling library modules (`logging`, `api`) that tag in lock-step
    // contribute no archive to the shared hosted release.
    for delegated in [false, true] {
        let root = TempDir::new().unwrap();
        let asset_rel = "dist/cli-x86_64-unknown-linux-gnu.tar.gz";
        let binary_path = root.path().join("bin/cli");
        let runner = if delegated {
            FakeToolRunner::new().with_produced_file(root.path().join(asset_rel), b"archive")
        } else {
            write_go_binary(root.path(), "bin/cli");
            FakeToolRunner::new()
        };
        let binary = if delegated {
            None
        } else {
            Some(binary_path.as_path())
        };
        let provider = go_provider(vec![asset_rel], delegated);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            binary,
            &runner,
            &mut reporter,
        )
        .expect("multi-module go package runs");

        assert_eq!(
            report.assets.len(),
            1,
            "only the binary module attaches an archive (delegated={delegated})"
        );
        assert_eq!(
            report.assets[0].backing,
            if delegated { "delegated" } else { "native" }
        );
    }
}
