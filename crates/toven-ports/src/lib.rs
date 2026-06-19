//! `toven-ports` — the port contracts every adapter (in-tree or 3rd-party)
//! implements, plus the fat helpers that make implementing them easy.
//!
//! Layer 2 of the hexagonal architecture: the thin traits ecosystems implement +
//! the shared surface behind them. Adapters build against `toven-ports`, never
//! against the engine. It depends only on [`toven_model`] (the shared
//! vocabulary), the error contract ([`rskit_errors`]), and the reuse primitives
//! it wraps ([`rskit_util`] templating, [`rskit_version`] semver).
//!
//! All fallible methods return [`rskit_errors::AppResult`]. Port traits are
//! object-safe so registries store trait objects (`dyn Provider`,
//! `dyn ConfiguredAdapter`, `dyn ReleaseTarget`, `dyn Reporter`, `dyn VcsReader`,
//! `dyn VcsWriter`).
//!
//! ## Ports
//! - [`provider`] — [`Provider`]/[`ConfiguredAdapter`]: the raw-TOML → configured
//!   adapter seam.
//! - [`release`] — [`ReleaseTarget`] and friends: the thin ecosystem release sliver.
//! - [`reporter`] — [`Reporter`]: the observability output port.
//! - [`vcs`] — [`VcsReader`]/[`VcsWriter`]: the single git seam.
//! - [`discover`] — the discovery request/response vocabulary.
//!
//! ## Shared surface
//! - [`task`] — the tasks vocabulary ([`Task`], [`TaskKind`], [`FanOut`], …).
//! - [`config`] — [`CommonEcosystemConfig`] (the `#[serde(flatten)]` target) + knobs.
//! - [`template`] — [`CommandTemplate`] argv rendering over rskit-util.
//! - [`merge`] — the [`merge_task`] field-merge helper.

pub mod config;
pub mod discover;
pub mod merge;
pub mod provider;
pub mod release;
pub mod reporter;
pub mod task;
pub mod template;
pub mod vcs;

pub use config::{CommonEcosystemConfig, ReleaseConfig, RunStrategy, TaskOverride};
pub use discover::{DISCOVERY_SCHEMA_VERSION, DiscoverContext, DiscoverRequest, DiscoverResponse};
pub use merge::merge_task;
pub use provider::{ConfiguredAdapter, EcosystemFragment, Provider};
pub use release::{Artifact, PublishOutcome, ReleaseMutation, ReleaseTarget};
pub use reporter::Reporter;
pub use task::{
    DEFAULT_READINESS_TIMEOUT, FanOut, Readiness, Task, TaskKind, TaskOrigin, ToolchainProbe,
};
pub use template::{CommandTemplate, TaskVar};
pub use vcs::{
    BaselineMode, BaselineSpec, ChangeRecord, ChangeStatus, Oid, TagRef, VcsReader, VcsWriter,
};

#[cfg(test)]
mod object_safety {
    //! Compile-time proof that every port trait is object-safe, with a trivial
    //! fake impl per trait so the engine can store them as trait objects.

    use std::path::Path;

    use rskit_errors::AppResult;
    use rskit_version::semver::Version;
    use toml::Table;
    use toven_model::{AbsPath, EcosystemId, Event, Module, ModuleRef, RepoPath};

    use super::*;

    struct FakeReporter;
    impl Reporter for FakeReporter {
        fn emit(&mut self, _event: &Event) -> AppResult<()> {
            Ok(())
        }
    }

    struct FakeReleaseTarget;
    impl ReleaseTarget for FakeReleaseTarget {
        fn declared_version(&self, _module: &Module) -> AppResult<Version> {
            Ok(Version::new(0, 1, 0))
        }
        fn published_versions(&self, _module: &Module) -> AppResult<Vec<Version>> {
            Ok(Vec::new())
        }
        fn package(&self, _module: &Module) -> AppResult<Artifact> {
            Ok(Artifact::new("dist/fake.crate"))
        }
        fn apply_release(&self, _module: &Module, _mutation: &ReleaseMutation) -> AppResult<()> {
            Ok(())
        }
        fn publish(&self, _module: &Module, _artifact: &Artifact) -> AppResult<PublishOutcome> {
            Ok(PublishOutcome::Published)
        }
    }

    struct FakeConfigured(CommonEcosystemConfig);
    impl ConfiguredAdapter for FakeConfigured {
        fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
            Ok(DiscoverResponse::new(
                EcosystemId::new("rust").expect("valid id"),
            ))
            .map(|mut response| {
                response.schema_version = request.schema_version;
                response
            })
        }
        fn default_tasks(&self) -> Vec<Task> {
            Vec::new()
        }
        fn toolchain_probe(&self) -> ToolchainProbe {
            ToolchainProbe::new("cargo", "cargo", vec!["--version".into()])
        }
        fn run_strategy_default(&self, _kind: &TaskKind) -> RunStrategy {
            RunStrategy::LeafToTop
        }
        fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>> {
            Ok(Some(Box::new(FakeReleaseTarget)))
        }
        fn common(&self) -> &CommonEcosystemConfig {
            &self.0
        }
    }

    struct FakeProvider(EcosystemId);
    impl Provider for FakeProvider {
        fn ecosystem_id(&self) -> &EcosystemId {
            &self.0
        }
        fn configure(&self, _raw: toml::Value) -> AppResult<Box<dyn ConfiguredAdapter>> {
            Ok(Box::new(FakeConfigured(CommonEcosystemConfig::default())))
        }
        fn scaffold(&self, _project_root: &Path) -> AppResult<Option<EcosystemFragment>> {
            Ok(Some(EcosystemFragment::new(self.0.clone(), Table::new())))
        }
    }

    struct FakeVcs;
    impl VcsReader for FakeVcs {
        fn rev_parse(&self, _rev: &str) -> AppResult<Oid> {
            Ok(Oid::new("deadbeef"))
        }
        fn merge_base(&self, _a: &str, _b: &str) -> AppResult<Oid> {
            Ok(Oid::new("deadbeef"))
        }
        fn list_tags(&self, _pattern: Option<&str>) -> AppResult<Vec<TagRef>> {
            Ok(Vec::new())
        }
        fn changed_since(&self, _spec: &BaselineSpec) -> AppResult<Vec<ChangeRecord>> {
            Ok(Vec::new())
        }
        fn worktree_status(&self) -> AppResult<Vec<ChangeRecord>> {
            Ok(Vec::new())
        }
        fn is_ignored(&self, _repo_relative: &Path) -> AppResult<bool> {
            Ok(false)
        }
    }
    impl VcsWriter for FakeVcs {
        fn commit(&self, _message: &str) -> AppResult<Oid> {
            Ok(Oid::new("deadbeef"))
        }
        fn create_tag(&self, _name: &str, _target: &str, _message: Option<&str>) -> AppResult<()> {
            Ok(())
        }
        fn push(&self, _refspecs: &[String]) -> AppResult<()> {
            Ok(())
        }
        fn restore_worktree(&self) -> AppResult<()> {
            Ok(())
        }
    }

    #[test]
    fn port_traits_are_object_safe() {
        let mut reporter: Box<dyn Reporter> = Box::new(FakeReporter);
        let release: Box<dyn ReleaseTarget> = Box::new(FakeReleaseTarget);
        let reader: Box<dyn VcsReader> = Box::new(FakeVcs);
        let writer: Box<dyn VcsWriter> = Box::new(FakeVcs);
        let provider: Box<dyn Provider> =
            Box::new(FakeProvider(EcosystemId::new("rust").expect("valid id")));

        // Exercise every Provider method.
        assert_eq!(provider.ecosystem_id().as_str(), "rust");
        assert!(
            provider
                .scaffold(Path::new("."))
                .expect("scaffolds")
                .is_some()
        );
        let configured = provider
            .configure(toml::Value::Table(Table::new()))
            .expect("configures");

        // Exercise every ConfiguredAdapter method.
        let module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("valid id"), "fake").expect("valid ref"),
            RepoPath::new("crates/fake").expect("valid path"),
        );
        let request = DiscoverRequest::new(AbsPath::new("/repo").expect("valid path"));
        let response = configured.discover(&request).expect("discovers");
        assert_eq!(response.schema_version, request.schema_version);
        assert!(configured.default_tasks().is_empty());
        assert_eq!(configured.toolchain_probe().label, "cargo");
        assert_eq!(
            configured.run_strategy_default(&TaskKind::Build),
            RunStrategy::LeafToTop
        );
        assert_eq!(configured.common(), &CommonEcosystemConfig::default());

        // Exercise every ReleaseTarget method (directly and via the adapter seam).
        let target = configured.release_target().expect("ok").expect("present");
        assert_eq!(target.declared_version(&module).expect("ok").minor, 1);
        assert!(target.published_versions(&module).expect("ok").is_empty());
        let artifact = target.package(&module).expect("packages");
        target
            .apply_release(&module, &ReleaseMutation::version(Version::new(1, 0, 0)))
            .expect("applies");
        assert_eq!(
            target.publish(&module, &artifact).expect("publishes"),
            PublishOutcome::Published
        );
        let direct_artifact = release.package(&module).expect("packages");
        assert_eq!(direct_artifact.path, artifact.path);

        // Exercise the Reporter port.
        reporter
            .emit(&Event::PlanPrepared { waves: 0, units: 0 })
            .expect("emits without error");

        // Exercise every VcsReader method.
        assert_eq!(reader.rev_parse("HEAD").expect("ok").as_str(), "deadbeef");
        assert_eq!(
            reader.merge_base("a", "b").expect("ok").as_str(),
            "deadbeef"
        );
        assert!(reader.list_tags(None).expect("ok").is_empty());
        let spec = BaselineSpec::explicit("main");
        assert!(reader.changed_since(&spec).expect("ok").is_empty());
        assert!(reader.worktree_status().expect("ok").is_empty());
        assert!(!reader.is_ignored(Path::new("target")).expect("ignored"));

        // Exercise every VcsWriter method.
        assert_eq!(writer.commit("msg").expect("ok").as_str(), "deadbeef");
        writer.create_tag("v1", "HEAD", Some("msg")).expect("tags");
        writer.push(&["refs/heads/main".into()]).expect("pushes");
        writer.restore_worktree().expect("restores");
    }
}
