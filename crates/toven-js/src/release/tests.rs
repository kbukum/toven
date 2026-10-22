use std::path::{Path, PathBuf};
use std::sync::Arc;

use rskit_errors::ErrorCode;
use rskit_fs::TempDir;
use rskit_version::semver::Version;
use toven_model::{AbsPath, Module};
use toven_ports::{
    Artifact, ForwardEnvAs, ManifestMutator, Packager, PublishOutcome, Publisher,
    ReleaseCredentials, ReleaseMutation, SbomProducer, TagGrammar, ToolOutcome, VersionSource,
    Visibility,
};
use toven_testkit::FakeToolRunner;

use super::target::{NpmRegistryTarget, publish_argv, token_injection};
use crate::config::JsConfig;
use crate::discovery;
use toven_ports::DiscoverRequest;

const NPM_ROOT: &str = r#"{
  "name": "monorepo",
  "private": true,
  "workspaces": ["packages/*"]
}
"#;

const PNPM_ROOT: &str = r#"{
  "name": "monorepo",
  "private": true,
  "packageManager": "pnpm@10.18.0"
}
"#;

const CORE: &str = r#"{
  "name": "@scope/core",
  "version": "1.0.0"
}
"#;

const APP: &str = r#"{
  "name": "@scope/app",
  "version": "2.0.0",
  "dependencies": {
    "@scope/core": "^1.0.0"
  }
}
"#;

/// A temporary npm or pnpm workspace with `@scope/core` and `@scope/app`.
struct Project {
    dir: TempDir,
    pack: TempDir,
    modules: Vec<Module>,
}

impl Project {
    fn npm(lockfile: bool) -> Self {
        let dir = TempDir::new().expect("temp dir");
        dir.write_file("package.json", NPM_ROOT.as_bytes())
            .expect("root");
        if lockfile {
            dir.write_file("package-lock.json", b"{\"lockfileVersion\": 3}\n")
                .expect("lockfile");
        }
        Self::with_members(dir)
    }

    fn pnpm() -> Self {
        let dir = TempDir::new().expect("temp dir");
        dir.write_file("package.json", PNPM_ROOT.as_bytes())
            .expect("root");
        dir.write_file("pnpm-workspace.yaml", b"packages:\n  - packages/*\n")
            .expect("workspace");
        Self::with_members(dir)
    }

    fn with_members(dir: TempDir) -> Self {
        dir.write_file("packages/core/package.json", CORE.as_bytes())
            .expect("core");
        dir.write_file("packages/app/package.json", APP.as_bytes())
            .expect("app");
        let root = AbsPath::new(dir.path().to_path_buf()).expect("absolute root");
        let modules = discovery::discover(&JsConfig::default(), &DiscoverRequest::new(root))
            .expect("discover")
            .modules;
        Self {
            dir,
            pack: TempDir::new().expect("pack dir"),
            modules,
        }
    }

    fn target(&self, runner: &FakeToolRunner) -> NpmRegistryTarget {
        self.target_with(runner, JsConfig::default())
    }

    fn target_with(&self, runner: &FakeToolRunner, config: JsConfig) -> NpmRegistryTarget {
        NpmRegistryTarget::new(
            config,
            Arc::new(runner.clone()),
            self.dir.path().to_path_buf(),
        )
        .with_pack_root(self.pack.path().to_path_buf())
    }

    fn module(&self, package: &str) -> &Module {
        self.modules
            .iter()
            .find(|module| module.package.as_deref() == Some(package))
            .unwrap_or_else(|| panic!("module {package}"))
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(path)).expect("read")
    }

    fn path(&self, path: &str) -> PathBuf {
        self.dir.path().join(path)
    }
}

fn argv(runner: &FakeToolRunner, index: usize) -> Vec<String> {
    runner.requests()[index].argv.clone()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn credentials(token_env: Option<&str>, registry: Option<&str>) -> ReleaseCredentials {
    ReleaseCredentials::new(token_env.map(String::from), registry.map(String::from))
}

#[test]
fn declared_version_reads_package_json() {
    let project = Project::npm(false);
    let runner = FakeToolRunner::new();
    let target = project.target(&runner);
    assert_eq!(
        target
            .declared_version(project.module("@scope/core"))
            .expect("version"),
        Some(Version::new(1, 0, 0))
    );
    assert!(runner.requests().is_empty());
}

#[test]
fn default_tag_is_package_at_version() {
    let project = Project::npm(false);
    let target = project.target(&FakeToolRunner::new());
    let scheme = target
        .tag_scheme(project.module("@scope/core"), None)
        .expect("scheme");
    assert_eq!(scheme.format(&Version::new(1, 2, 3)), "@scope/core@1.2.3");
}

#[test]
fn published_versions_queries_npm_in_the_root() {
    let project = Project::npm(false);
    let runner = FakeToolRunner::new().with_stdout(r#"["0.9.0", "1.0.0"]"#);
    let target = project.target(&runner);
    let versions = target
        .published_versions(project.module("@scope/core"))
        .expect("versions");
    assert_eq!(versions, vec![Version::new(0, 9, 0), Version::new(1, 0, 0)]);
    let request = &runner.requests()[0];
    assert_eq!(
        request.argv,
        strings(&["npm", "view", "@scope/core", "versions", "--json"])
    );
    assert_eq!(request.working_dir.as_deref(), Some(project.dir.path()));
}

#[test]
fn apply_release_rewrites_version_and_sibling_range() {
    let project = Project::npm(false);
    let target = project.target(&FakeToolRunner::new());
    let mut mutation = ReleaseMutation::version(Version::new(2, 1, 0));
    mutation
        .dep_floor_import_updates
        .insert("@scope/core".to_string(), Version::new(1, 1, 0));
    let written = target
        .apply_release(project.module("@scope/app"), &mutation)
        .expect("apply");
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].as_path(), Path::new("packages/app/package.json"));
    assert_eq!(
        project.read("packages/app/package.json"),
        APP.replace("2.0.0", "2.1.0").replace("^1.0.0", "^1.1.0")
    );
}

#[test]
fn apply_release_without_a_change_writes_nothing() {
    let project = Project::npm(false);
    let target = project.target(&FakeToolRunner::new());
    let written = target
        .apply_release(
            project.module("@scope/core"),
            &ReleaseMutation::version(Version::new(1, 0, 0)),
        )
        .expect("apply");
    assert!(written.is_empty());
}

#[test]
fn finalize_refreshes_a_changed_lockfile_once_per_root() {
    let project = Project::npm(true);
    let runner = FakeToolRunner::new().with_produced_file(
        project.path("package-lock.json"),
        b"{\"refreshed\": true}\n",
    );
    let target = project.target(&runner);
    let written = target
        .finalize_release(&[project.module("@scope/core"), project.module("@scope/app")])
        .expect("finalize");
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].as_path(), Path::new("package-lock.json"));
    assert_eq!(runner.requests().len(), 1);
    assert_eq!(
        argv(&runner, 0),
        strings(&["npm", "install", "--package-lock-only", "--ignore-scripts"])
    );
}

#[test]
fn finalize_skips_an_unchanged_lockfile_and_a_root_without_one() {
    let locked = Project::npm(true);
    let runner = FakeToolRunner::new();
    let written = locked
        .target(&runner)
        .finalize_release(&[locked.module("@scope/core")])
        .expect("finalize");
    assert!(written.is_empty());
    assert_eq!(runner.requests().len(), 1);

    let unlocked = Project::npm(false);
    let runner = FakeToolRunner::new();
    let written = unlocked
        .target(&runner)
        .finalize_release(&[unlocked.module("@scope/core")])
        .expect("finalize");
    assert!(written.is_empty());
    assert!(runner.requests().is_empty());
}

#[test]
fn finalize_fails_when_the_refresh_fails() {
    let project = Project::npm(true);
    let runner = FakeToolRunner::new().with_exit_code(Some(1));
    let error = project
        .target(&runner)
        .finalize_release(&[project.module("@scope/core")])
        .expect_err("refresh failure");
    assert_eq!(error.code(), ErrorCode::ExternalService);
}

#[test]
fn package_packs_with_the_root_manager_in_the_member_dir() {
    let project = Project::pnpm();
    let destination = project.pack.path().join("packages-core-1.0.0");
    let runner = FakeToolRunner::new()
        .with_produced_file(destination.join("scope-core-1.0.0.tgz"), b"tarball");
    let artifact = project
        .target(&runner)
        .package(project.module("@scope/core"))
        .expect("package");
    assert_eq!(artifact.path, destination.join("scope-core-1.0.0.tgz"));
    let request = &runner.requests()[0];
    assert_eq!(
        request.argv,
        vec![
            "pnpm".to_string(),
            "pack".to_string(),
            "--pack-destination".to_string(),
            destination.display().to_string(),
        ]
    );
    assert_eq!(
        request.working_dir.as_deref(),
        Some(project.path("packages/core").as_path())
    );
}

#[test]
fn package_requires_exactly_one_tarball() {
    let project = Project::npm(false);
    let runner = FakeToolRunner::new();
    let error = project
        .target(&runner)
        .package(project.module("@scope/core"))
        .expect_err("no tarball");
    assert_eq!(error.code(), ErrorCode::ExternalService);

    let destination = project.pack.path().join("packages-core-1.0.0");
    let runner = FakeToolRunner::new()
        .with_produced_file(destination.join("a.tgz"), b"a")
        .with_produced_file(destination.join("b.tgz"), b"b");
    let error = project
        .target(&runner)
        .package(project.module("@scope/core"))
        .expect_err("two tarballs");
    assert_eq!(error.code(), ErrorCode::ExternalService);
}

#[test]
fn package_fails_when_the_pack_fails() {
    let project = Project::npm(false);
    let runner = FakeToolRunner::new()
        .with_exit_code(Some(1))
        .with_stderr("prepack failed");
    let error = project
        .target(&runner)
        .package(project.module("@scope/core"))
        .expect_err("pack failure");
    assert_eq!(error.code(), ErrorCode::ExternalService);
}

#[test]
fn package_runs_the_root_pack_command_from_the_project_root() {
    let project = Project::npm(false);
    let destination = project.pack.path().join("packages-core-1.0.0");
    let runner = FakeToolRunner::new()
        .with_produced_file(destination.join("scope-core-1.0.0.tgz"), b"tarball");
    let artifact = project
        .target_with(&runner, pack_config("."))
        .package(project.module("@scope/core"))
        .expect("package");
    assert_eq!(artifact.path, destination.join("scope-core-1.0.0.tgz"));
    let request = &runner.requests()[0];
    assert_eq!(
        request.argv,
        vec![
            "bun".to_string(),
            "scripts/pack.ts".to_string(),
            "packages/core".to_string(),
            format!("--destination={}", destination.display()),
        ]
    );
    assert_eq!(
        request.working_dir.as_deref(),
        Some(project.path("").as_path())
    );
    assert!(request.timeout.is_some(), "the pack command is bounded");
}

#[test]
fn package_with_a_pack_command_still_requires_exactly_one_tarball() {
    let project = Project::npm(false);
    let error = project
        .target_with(&FakeToolRunner::new(), pack_config("."))
        .package(project.module("@scope/core"))
        .expect_err("no tarball");
    assert_eq!(error.code(), ErrorCode::ExternalService);
    assert!(error.to_string().contains("`bun`"), "{error}");

    let runner = FakeToolRunner::new().with_exit_code(Some(1));
    let error = project
        .target_with(&runner, pack_config("."))
        .package(project.module("@scope/core"))
        .expect_err("pack failure");
    assert_eq!(error.code(), ErrorCode::ExternalService);
}

#[test]
fn package_rejects_a_pack_command_for_an_unknown_root() {
    let project = Project::npm(false);
    let runner = FakeToolRunner::new();
    let error = project
        .target_with(&runner, pack_config("web"))
        .package(project.module("@scope/core"))
        .expect_err("unknown root");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(error.to_string().contains("ecosystems.js.pack"), "{error}");
    assert!(runner.requests().is_empty());
}

/// A config whose `root` packs with `bun scripts/pack.ts`.
fn pack_config(root: &str) -> JsConfig {
    JsConfig {
        pack: [(
            root.to_string(),
            strings(&[
                "bun",
                "scripts/pack.ts",
                "{module.root}",
                "--destination={destination}",
            ]),
        )]
        .into(),
        ..JsConfig::default()
    }
}

#[test]
fn package_rejects_a_prerelease_without_a_dist_tag_before_packing() {
    let project = Project::npm(false);
    std::fs::write(
        project.path("packages/core/package.json"),
        CORE.replace("1.0.0", "1.1.0-1"),
    )
    .expect("write");
    let runner = FakeToolRunner::new();
    let error = project
        .target(&runner)
        .package(project.module("@scope/core"))
        .expect_err("numeric prerelease");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(runner.requests().is_empty());
}

#[test]
fn publish_argv_names_registry_tag_and_access() {
    assert_eq!(
        publish_argv(
            Path::new("/p/core.tgz"),
            "https://registry.npmjs.org/",
            "next",
            "public"
        ),
        strings(&[
            "npm",
            "publish",
            "/p/core.tgz",
            "--registry",
            "https://registry.npmjs.org/",
            "--tag",
            "next",
            "--access",
            "public",
        ])
    );
}

#[test]
fn token_injection_maps_token_env_to_the_registry_auth_variable() {
    let mapping = token_injection(
        &credentials(Some("NPM_TOKEN"), None),
        "https://registry.npmjs.org/",
        |_| Some("secret".to_string()),
    )
    .expect("mapping");
    assert_eq!(
        mapping,
        Some(ForwardEnvAs::new(
            "NPM_TOKEN",
            "npm_config_//registry.npmjs.org/:_authToken"
        ))
    );
    assert_eq!(
        token_injection(
            &credentials(None, None),
            "https://registry.npmjs.org/",
            |_| { panic!("no token env to read") }
        )
        .expect("ambient"),
        None
    );
}

#[test]
fn token_injection_fails_closed_when_the_variable_is_unset() {
    let error = token_injection(
        &credentials(Some("NPM_TOKEN"), None),
        "https://registry.npmjs.org/",
        |_| None,
    )
    .expect_err("unset token");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(!error.to_string().contains("secret"));
}

#[test]
fn publish_runs_npm_with_the_dist_tag_from_the_version() {
    let project = Project::npm(false);
    std::fs::write(
        project.path("packages/core/package.json"),
        CORE.replace("1.0.0", "1.1.0-beta.2"),
    )
    .expect("write");
    let runner = FakeToolRunner::new();
    let outcome = project
        .target(&runner)
        .publish(
            project.module("@scope/core"),
            &Artifact::new("/p/core.tgz"),
            &credentials(None, None),
            Visibility::Private,
        )
        .expect("publish");
    assert_eq!(outcome, PublishOutcome::Published);
    let request = &runner.requests()[0];
    assert_eq!(
        request.argv,
        strings(&[
            "npm",
            "publish",
            "/p/core.tgz",
            "--registry",
            "https://registry.npmjs.org/",
            "--tag",
            "beta",
            "--access",
            "restricted",
        ])
    );
    assert_eq!(request.working_dir.as_deref(), Some(project.dir.path()));
    assert!(request.forward_env_as.is_empty());
}

#[test]
fn publish_classifies_timeouts_and_auth_failures() {
    let project = Project::npm(false);
    let publish = |runner: &FakeToolRunner| {
        project.target(runner).publish(
            project.module("@scope/core"),
            &Artifact::new("/p/core.tgz"),
            &credentials(None, Some("npmjs")),
            Visibility::Public,
        )
    };
    let timed_out = FakeToolRunner::new()
        .with_exit_code(None)
        .with_timed_out(true);
    assert_eq!(
        publish(&timed_out).expect_err("timeout").code(),
        ErrorCode::Timeout
    );
    let unauthorized = FakeToolRunner::new()
        .with_exit_code(Some(1))
        .with_stderr("npm error code E401\nnpm error 401 Unauthorized");
    assert_eq!(
        publish(&unauthorized).expect_err("auth").code(),
        ErrorCode::Unauthorized
    );
    let duplicate = FakeToolRunner::new().with_outcomes([ToolOutcome::new(
        Some(1),
        String::new(),
        "npm error code E403\nnpm error 403 You cannot publish over the previously published versions: 1.0.0."
            .to_string(),
    )]);
    assert_eq!(
        publish(&duplicate).expect("already published"),
        PublishOutcome::AlreadyPublished
    );
}

#[test]
fn validate_publication_rejects_unpublishable_settings() {
    let project = Project::npm(false);
    let target = project.target(&FakeToolRunner::new());
    let core = project.module("@scope/core");
    target
        .validate_publication(core, "npmjs", Visibility::Public)
        .expect("npmjs public");
    target
        .validate_publication(core, "https://npm.pkg.github.com/", Visibility::Private)
        .expect("scoped private");
    for (registry, visibility) in [
        ("http://registry.example.com/", Visibility::Public),
        (
            "https://user:pass@registry.example.com/",
            Visibility::Public,
        ),
        ("npmjs", Visibility::Internal),
    ] {
        let error = target
            .validate_publication(core, registry, visibility)
            .expect_err(registry);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{registry}");
    }
}

#[test]
fn sbom_uses_npm_with_the_member_workspace() {
    let project = Project::npm(false);
    let runner = FakeToolRunner::new().with_stdout("{\"bomFormat\":\"CycloneDX\"}");
    let out = TempDir::new().expect("out");
    let artifact = project
        .target(&runner)
        .sbom(project.module("@scope/core"), out.path())
        .expect("sbom")
        .expect("npm sbom");
    assert_eq!(artifact.path, out.path().join("packages-core.cdx.json"));
    assert_eq!(
        std::fs::read_to_string(&artifact.path).expect("read"),
        "{\"bomFormat\":\"CycloneDX\"}"
    );
    assert_eq!(
        argv(&runner, 0),
        strings(&[
            "npm",
            "sbom",
            "--sbom-format",
            "cyclonedx",
            "--omit",
            "dev",
            "--workspace",
            "packages/core",
        ])
    );
}

#[test]
fn sbom_is_absent_for_other_managers() {
    let project = Project::pnpm();
    let runner = FakeToolRunner::new();
    let out = TempDir::new().expect("out");
    assert!(
        project
            .target(&runner)
            .sbom(project.module("@scope/core"), out.path())
            .expect("sbom")
            .is_none()
    );
    assert!(runner.requests().is_empty());
}
