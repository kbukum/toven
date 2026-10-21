//! Behavioral discovery tests for the JavaScript adapter against testkit
//! fixture workspaces. Every adapter gets a recording tool runner, and
//! discovery must leave it unused: discovery never spawns a process.

use std::sync::Arc;

use rskit_errors::{AppResult, ErrorCode};
use toven_js::JsProvider;
use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleRef};
use toven_ports::{ConfiguredAdapter, DiscoverRequest, DiscoverResponse, Provider};
use toven_testkit::{FakeToolRunner, fixtures};

fn configure_with(adapter_config: &str, runner: &FakeToolRunner) -> Box<dyn ConfiguredAdapter> {
    let text = fixtures::ecosystem_string("js", adapter_config).expect("adapter fixture");
    JsProvider::new(Arc::new(runner.clone()))
        .expect("provider")
        .configure(fixtures::raw_subtree(&text).expect("raw subtree"))
        .expect("configure")
}

fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    configure_with(adapter_config, &FakeToolRunner::new())
}

fn discover_result(adapter_config: &str, workspace: &str) -> AppResult<DiscoverResponse> {
    let root = fixtures::ecosystem("js", format!("workspaces/{workspace}")).expect("workspace");
    let request = DiscoverRequest::new(AbsPath::new(root).expect("absolute root"));
    let runner = FakeToolRunner::new();
    let response = configure_with(adapter_config, &runner).discover(&request);
    assert!(runner.requests().is_empty(), "discovery spawned a process");
    response
}

fn discover(adapter_config: &str, workspace: &str) -> DiscoverResponse {
    discover_result(adapter_config, workspace).expect("discover")
}

fn module_ref(name: &str) -> ModuleRef {
    ModuleRef::new(EcosystemId::new("js").expect("id"), name).expect("ref")
}

fn module<'a>(response: &'a DiscoverResponse, name: &str) -> &'a Module {
    response
        .modules
        .iter()
        .find(|module| module.id == module_ref(name))
        .unwrap_or_else(|| panic!("module {name}"))
}

fn names(response: &DiscoverResponse) -> Vec<String> {
    response
        .modules
        .iter()
        .map(|module| module.id.name.clone())
        .collect()
}

fn edge(from: &str, to: &str, kind: DepKind) -> Edge {
    Edge::new(module_ref(from), module_ref(to), kind)
}

fn workspaces(response: &DiscoverResponse) -> Vec<(String, String, String)> {
    response
        .workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.id.as_str().to_string(),
                workspace.root.as_path().to_string_lossy().into_owned(),
                workspace.toolchain.tool.clone(),
            )
        })
        .collect()
}

#[test]
fn npm_workspace_links_satisfied_ranges() {
    let response = discover("adapter/auto.toml", "npm");

    assert_eq!(
        names(&response),
        [
            "packages-app",
            "packages-core",
            "packages-legacy",
            "packages-testing",
            "platform"
        ]
    );
    assert_eq!(
        response.edges,
        [
            edge("packages-app", "packages-core", DepKind::Normal),
            edge("packages-app", "packages-testing", DepKind::Dev),
        ]
    );
    assert_eq!(
        workspaces(&response),
        [("js".into(), ".".into(), "npm".into())]
    );
    assert_eq!(response.workspaces[0].toolchain.version, None);
    assert_eq!(
        response.workspaces[0].blast_radius,
        [
            "package.json",
            ".npmrc",
            "package-lock.json",
            "npm-shrinkwrap.json"
        ]
    );

    let core = module(&response, "packages-core");
    assert_eq!(core.package.as_deref(), Some("@acme/core"));
    assert_eq!(core.root.as_path().to_string_lossy(), "packages/core");
    assert_eq!(
        core.manifest
            .as_ref()
            .map(|path| path.as_path().to_string_lossy().into_owned()),
        Some("packages/core/package.json".into())
    );
    assert!(core.cache_inputs_complete);
    assert!(!core.unpublishable);
    let scripts: Vec<&str> = core
        .capabilities
        .iter()
        .map(toven_model::Capability::as_str)
        .collect();
    assert_eq!(scripts, ["script:build", "script:test"]);

    let root = module(&response, "platform");
    let root_scripts: Vec<&str> = root
        .capabilities
        .iter()
        .map(toven_model::Capability::as_str)
        .collect();
    assert_eq!(
        root_scripts,
        ["script:lint"],
        "a root script a member also declares delegates to the members"
    );
    assert_eq!(root.root.as_path().to_string_lossy(), ".");
    assert!(
        root.unpublishable,
        "a `private: true` package is never published"
    );
    assert_eq!(
        root.workspace
            .as_ref()
            .map(toven_model::WorkspaceId::as_str),
        Some("js")
    );
}

#[test]
fn pnpm_workspace_links_workspace_protocol_and_aliases() {
    let response = discover("adapter/auto.toml", "pnpm");

    assert_eq!(names(&response), ["apps-web", "libs-shared-fmt", "libs-ui"]);
    assert_eq!(
        response.edges,
        [
            edge("apps-web", "libs-ui", DepKind::Normal),
            edge("libs-shared-fmt", "libs-ui", DepKind::Normal),
        ]
    );
    assert_eq!(
        workspaces(&response),
        [("js".into(), ".".into(), "pnpm".into())]
    );
    assert!(
        response.workspaces[0]
            .blast_radius
            .contains(&"pnpm-workspace.yaml".to_string())
    );
}

#[test]
fn independent_roots_get_their_own_workspace_and_manager() {
    let response = discover("adapter/auto.toml", "multi-root");

    assert_eq!(
        names(&response),
        ["docs", "examples-stray", "tools-gen", "web"]
    );
    assert_eq!(
        workspaces(&response),
        [
            ("js:docs".into(), "docs".into(), "bun".into()),
            (
                "js:examples/stray".into(),
                "examples/stray".into(),
                "npm".into()
            ),
            ("js:tools/gen".into(), "tools/gen".into(), "yarn".into()),
            ("js:web".into(), "web".into(), "yarn".into()),
        ]
    );
    assert!(
        response.workspaces[3]
            .blast_radius
            .contains(&"web/.yarnrc.yml".to_string())
    );
    assert_eq!(response.edges, [edge("docs", "web", DepKind::Normal)]);

    let web = module(&response, "web");
    assert_eq!(web.external_roots, ["vendor/std-1.0.0.tgz"]);
    assert!(web.cache_inputs_complete);

    let docs = module(&response, "docs");
    assert!(!docs.cache_inputs_complete);
    assert!(
        response
            .warnings
            .iter()
            .any(|warning| warning.contains("'../gone'") || warning.contains("'gone'")),
        "{:?}",
        response.warnings
    );
}

#[test]
fn exclude_prunes_roots() {
    let response = discover("adapter/exclude-examples.toml", "multi-root");
    assert_eq!(names(&response), ["docs", "tools-gen", "web"]);
}

#[test]
fn explicit_roots_freeze_the_root_set() {
    let response = discover("adapter/explicit-roots.toml", "multi-root");
    assert_eq!(names(&response), ["docs", "web"]);
}

#[test]
fn explicit_root_that_is_also_a_member_conflicts() {
    let error = discover_result("adapter/explicit-member-root.toml", "pnpm").expect_err("conflict");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert!(error.to_string().contains("libs/ui"), "{error}");
}

#[test]
fn package_manager_override_wins() {
    let response = discover("adapter/package-manager-override.toml", "multi-root");
    let gen_root = response
        .workspaces
        .iter()
        .find(|workspace| workspace.id.as_str() == "js:tools/gen")
        .expect("tools/gen workspace");
    assert_eq!(gen_root.toolchain.tool, "npm");
}

#[test]
fn package_manager_conflicts_are_errors() {
    for workspace in ["conflicting-lockfiles", "stale-lockfile"] {
        let error = discover_result("adapter/auto.toml", workspace).expect_err(workspace);
        assert_eq!(error.code(), ErrorCode::Conflict, "{workspace}: {error}");
        assert!(
            error.to_string().contains("lockfile"),
            "{workspace}: {error}"
        );
    }
}

#[test]
fn malformed_manifest_names_the_file() {
    let error = discover_result("adapter/auto.toml", "malformed").expect_err("malformed");
    assert_eq!(error.code(), ErrorCode::InvalidFormat);
    assert!(error.to_string().contains("package.json"), "{error}");
}

#[test]
fn escaping_workspace_pattern_is_rejected() {
    let error = discover_result("adapter/auto.toml", "escaping-pattern").expect_err("escape");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(error.to_string().contains("escapes"), "{error}");
}

#[test]
fn duplicate_package_names_conflict() {
    let error = discover_result("adapter/auto.toml", "duplicate-name").expect_err("duplicate");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert!(error.to_string().contains("'dup'"), "{error}");
}

#[test]
fn missing_workspace_target_is_not_found() {
    let error = discover_result("adapter/auto.toml", "missing-workspace").expect_err("missing");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert!(error.to_string().contains("'lib'"), "{error}");
}

#[test]
fn oversized_manifest_is_rejected() {
    let dir = rskit_fs::TempDir::new().expect("tempdir");
    let padding = " ".repeat(1024 * 1024 + 1);
    dir.write_file(
        "package.json",
        format!("{{\"name\":\"big\"}}{padding}").as_bytes(),
    )
    .expect("write");
    let request = DiscoverRequest::new(AbsPath::new(dir.path().to_path_buf()).expect("root"));
    let error = configure("adapter/auto.toml")
        .discover(&request)
        .expect_err("oversized");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(error.to_string().contains("limit"), "{error}");
}

#[test]
fn toolchain_probes_follow_the_workspace_tool() {
    let adapter = configure("adapter/auto.toml");
    let intent = toven_ports::TaskIntent::resolve("build");
    let probes = |tool: &str| -> Vec<String> {
        adapter
            .toolchain_probes_for(&toven_model::ToolchainTag::new(tool), &intent)
            .expect("probes")
            .into_iter()
            .map(|probe| probe.program)
            .collect()
    };
    assert_eq!(probes("pnpm"), ["node", "pnpm"]);
    assert_eq!(probes("bun"), ["bun"]);
    assert_eq!(adapter.toolchain_probe().program, "node");
}

#[test]
fn the_crate_has_no_process_seam() {
    // The `structure` gate forbids raw spawns in production source; this
    // guards the other path: depending on a crate that owns a spawn seam.
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read manifest");
    let manifest: toml::Table = manifest.parse().expect("parse manifest");
    let dependencies = manifest["dependencies"].as_table().expect("dependencies");
    for seam in ["rskit-process", "toven-exec", "toven-core"] {
        assert!(
            !dependencies.contains_key(seam),
            "toven-js must not depend on {seam}"
        );
    }
}
