//! `toven-ports` — the port contracts every adapter (in-tree or 3rd-party)
//! implements, plus the fat helpers that make implementing them easy.
//!
//! Layer 1 of the hexagonal architecture: the thin traits ecosystems implement +
//! the shared surface behind them. Adapters build against `toven-ports`, never
//! against the engine. It depends only on [`toven_model`] (the shared
//! vocabulary), the error contract ([`rskit_errors`]), and the reuse primitives
//! it wraps ([`rskit_util`] templating, [`rskit_version`] semver).
//!
//! All fallible methods return [`rskit_errors::AppResult`]. Port traits are
//! object-safe so registries store trait objects (`dyn Provider`,
//! `dyn ConfiguredAdapter`, `dyn ReleaseTarget`, `dyn Reporter`,
//! `dyn RawOutputSink`, `dyn VcsReader`, `dyn VcsWriter`, `dyn ToolchainProber`,
//! `dyn SourceDigest`, `dyn CacheStore`).
//!
//! ## Ports
//! - [`provider`] — [`Provider`]/[`ConfiguredAdapter`]: the raw-TOML → configured
//!   adapter seam, plus the [`wizard`] onboarding steps.
//! - [`release`] — [`ReleaseTarget`] and friends: the thin ecosystem release sliver.
//! - [`reporter`] — [`Reporter`]: the observability output port.
//! - [`raw_output`] — [`RawOutputSink`]: the raw child-output sink port (sibling
//!   of [`Reporter`]; fed by the engine's `UnitOutputChannel`).
//! - [`vcs`] — [`VcsReader`]/[`VcsWriter`]: the single git seam.
//! - [`watch`] — [`WatchSource`]: the injected filesystem-watch seam (concrete
//!   rskit-fs adapter lives in the engine).
//! - [`toolchain`] — [`ToolchainProber`]: the injected toolchain-probe seam
//!   (concrete subprocess prober lives in the engine).
//! - [`source`] — [`SourceDigest`]: the injected content-digest seam (concrete
//!   filesystem digest lives in the engine).
//! - [`cache`] — [`CacheStore`] (read, PLAN) + [`CacheWriter`] (write, APPLY):
//!   the injected cache-record seam (concrete backend lives in the engine).
//! - [`exec`] — [`CommandRunner`]: the injected process-execution seam consumed
//!   by the APPLY wave walk (concrete `rskit-process` runner lives in the engine).
//! - [`discover`] — the discovery request/response vocabulary.
//! - [`driver`] — [`DriverLocator`]/[`DriverWizard`]: the out-of-process
//!   `toven-<eco>` driver seams (concrete adapters live in the engine).
//!
//! ## Shared surface
//! - [`task`] — the tasks vocabulary ([`Task`], [`TaskKind`], [`FanOut`], …).
//! - [`config`] — [`CommonEcosystemConfig`] (the `#[serde(flatten)]` target) + knobs.
//! - [`wizard`] — the data-only onboarding vocabulary ([`Detection`],
//!   [`Questionnaire`], [`Answers`]).
//! - [`template`] — [`CommandTemplate`] argv rendering over rskit-util.
//! - [`merge`] — the [`merge_task`] field-merge helper.

pub mod cache;
pub mod config;
pub mod discover;
pub mod driver;
pub mod exec;
pub mod merge;
pub mod provider;
pub mod raw_output;
pub mod release;
pub mod reporter;
pub mod source;
pub mod task;
pub mod template;
pub mod toolchain;
pub mod vcs;
pub mod watch;
pub mod wizard;

pub use cache::{CacheStore, CacheWriter};
pub use config::{CommonEcosystemConfig, ReleaseConfig, RunStrategy, TaskEntry, TaskOverride};
pub use discover::{DISCOVERY_SCHEMA_VERSION, DiscoverContext, DiscoverRequest, DiscoverResponse};
pub use driver::{DriverLocator, DriverWizard};
pub use exec::{
    CommandRunner, HeldProcess, Invocation, InvocationEnvPolicy, InvocationEnvironment,
    OutputObserver, RunOutcome, StartOutcome,
};
pub use merge::merge_task;
pub use provider::{ConfiguredAdapter, EcosystemFragment, Provider};
pub use raw_output::RawOutputSink;
pub use release::{Artifact, PublishOutcome, RegistryCadence, ReleaseMutation, ReleaseTarget};
pub use reporter::Reporter;
pub use source::SourceDigest;
pub use task::{
    DEFAULT_READINESS_TIMEOUT, FanOut, Readiness, Task, TaskIntent, TaskKind, TaskOrigin,
    ToolchainProbe,
};
pub use template::{CommandTemplate, TaskVar};
pub use toolchain::ToolchainProber;
pub use vcs::{
    BaselineMode, BaselineSpec, ChangeRecord, ChangeStatus, Oid, TagRef, VcsReader, VcsWriter,
};
pub use watch::{ChangeBatch, ChangeBatchStream, WatchSource};
pub use wizard::{
    Answer, AnswerProvider, Answers, Detection, Question, QuestionId, QuestionKind, Questionnaire,
    TextRule,
};

#[cfg(test)]
mod object_safety {
    //! Compile-time proof that every port trait is object-safe, with a trivial
    //! fake impl per trait so the engine can store them as trait objects.

    use std::path::Path;

    use rskit_errors::AppResult;
    use rskit_version::semver::Version;
    use toml::Table;
    use toven_model::{
        AbsPath, EcosystemId, Event, Module, ModuleRef, OutputStream, RepoPath, UnitOutput,
    };

    use super::*;

    struct FakeReporter;
    impl Reporter for FakeReporter {
        fn emit(&mut self, _event: &Event) -> AppResult<()> {
            Ok(())
        }
    }

    struct FakeRawOutputSink {
        live: usize,
        blocks: usize,
    }
    impl RawOutputSink for FakeRawOutputSink {
        fn live(&mut self, _chunk: &UnitOutput) -> AppResult<()> {
            self.live += 1;
            Ok(())
        }
        fn block(&mut self, _unit_id: &str, _chunks: &[UnitOutput]) -> AppResult<()> {
            self.blocks += 1;
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
        fn toolchain_probe(&self) -> ToolchainProbe {
            ToolchainProbe::new("cargo", "cargo", vec!["--version".into()])
        }
        fn run_strategy_default(&self, _kind: TaskKind) -> RunStrategy {
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
        fn configure(&self, _raw: rskit_config::RawValue) -> AppResult<Box<dyn ConfiguredAdapter>> {
            Ok(Box::new(FakeConfigured(CommonEcosystemConfig::default())))
        }
        fn detect(&self, _project_root: &Path) -> AppResult<Option<wizard::Detection>> {
            Ok(Some(wizard::Detection::bare(self.0.clone())))
        }
        fn questionnaire(&self, detection: &wizard::Detection) -> AppResult<wizard::Questionnaire> {
            Ok(wizard::Questionnaire::empty(detection.ecosystem.clone()))
        }
        fn render(
            &self,
            detection: &wizard::Detection,
            _answers: &wizard::Answers,
        ) -> AppResult<EcosystemFragment> {
            Ok(EcosystemFragment::new(
                detection.ecosystem.clone(),
                Table::new(),
            ))
        }
    }

    struct FakeToolchainProber;
    impl ToolchainProber for FakeToolchainProber {
        fn probe(&self, _probe: &ToolchainProbe, _workspace_root: &Path) -> AppResult<String> {
            Ok("v1".to_string())
        }
    }

    struct FakeSourceDigest;
    impl SourceDigest for FakeSourceDigest {
        fn module(&self, module: &Module) -> AppResult<String> {
            Ok(format!("module:{}", module.id))
        }
        fn path(&self, repo_relative: &Path) -> AppResult<String> {
            Ok(format!("path:{}", repo_relative.display()))
        }
    }

    struct FakeCacheStore;
    impl CacheStore for FakeCacheStore {
        fn contains(&self, _key: &str) -> AppResult<bool> {
            Ok(false)
        }
    }

    struct FakeCacheWriter;
    impl CacheWriter for FakeCacheWriter {
        fn record(&self, _key: &str) -> AppResult<()> {
            Ok(())
        }
    }

    struct FakeHeldProcess;
    impl HeldProcess for FakeHeldProcess {
        fn unit_id(&self) -> &'static str {
            "rust:fake#run"
        }
        fn shutdown(self: Box<Self>) -> AppResult<()> {
            Ok(())
        }
    }

    struct FakeCommandRunner;
    #[async_trait::async_trait]
    impl CommandRunner for FakeCommandRunner {
        async fn run(
            &self,
            _invocation: &Invocation,
            _cancel: tokio_util::sync::CancellationToken,
            _live: Option<OutputObserver>,
        ) -> AppResult<RunOutcome> {
            Ok(RunOutcome::succeeded(Vec::new()))
        }
        async fn start_persistent(
            &self,
            _invocation: &Invocation,
            _cancel: tokio_util::sync::CancellationToken,
            _output: OutputObserver,
        ) -> AppResult<StartOutcome> {
            Ok(StartOutcome::Ready {
                output: Vec::new(),
                process: Box::new(FakeHeldProcess),
            })
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
        fn create_tag(
            &self,
            _name: &str,
            _target_rev: &str,
            _message: Option<&str>,
        ) -> AppResult<()> {
            Ok(())
        }
        fn push(&self, _refspecs: &[String]) -> AppResult<()> {
            Ok(())
        }
        fn restore_worktree(&self) -> AppResult<()> {
            Ok(())
        }
    }

    struct FakeWatchSource;
    impl WatchSource for FakeWatchSource {
        fn changes(
            &self,
            _roots: &[AbsPath],
            _debounce: std::time::Duration,
            _cancel: tokio_util::sync::CancellationToken,
        ) -> AppResult<ChangeBatchStream> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn port_traits_are_object_safe() {
        let mut reporter: Box<dyn Reporter> = Box::new(FakeReporter);
        let mut raw_sink: Box<dyn RawOutputSink> =
            Box::new(FakeRawOutputSink { live: 0, blocks: 0 });
        let release: Box<dyn ReleaseTarget> = Box::new(FakeReleaseTarget);
        let reader: Box<dyn VcsReader> = Box::new(FakeVcs);
        let writer: Box<dyn VcsWriter> = Box::new(FakeVcs);
        let prober: Box<dyn ToolchainProber> = Box::new(FakeToolchainProber);
        let digest: Box<dyn SourceDigest> = Box::new(FakeSourceDigest);
        let cache: Box<dyn CacheStore> = Box::new(FakeCacheStore);
        let cache_writer: Box<dyn CacheWriter> = Box::new(FakeCacheWriter);
        let runner: Box<dyn CommandRunner> = Box::new(FakeCommandRunner);
        let held: Box<dyn HeldProcess> = Box::new(FakeHeldProcess);
        let provider: Box<dyn Provider> =
            Box::new(FakeProvider(EcosystemId::new("rust").expect("valid id")));

        // Exercise every Provider method.
        assert_eq!(provider.ecosystem_id().as_str(), "rust");
        let detection = provider
            .detect(Path::new("."))
            .expect("detects")
            .expect("present");
        assert_eq!(detection.ecosystem.as_str(), "rust");
        let questionnaire = provider.questionnaire(&detection).expect("questionnaire");
        assert!(questionnaire.is_empty());
        let fragment = provider
            .render(&detection, &wizard::Answers::new())
            .expect("renders");
        assert_eq!(fragment.ecosystem.as_str(), "rust");
        let configured = provider
            .configure(rskit_config::RawValue::Null)
            .expect("configures");

        // Exercise every ConfiguredAdapter method.
        let module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("valid id"), "fake").expect("valid ref"),
            RepoPath::new("crates/fake").expect("valid path"),
        );
        let request = DiscoverRequest::new(AbsPath::new("/repo").expect("valid path"));
        let response = configured.discover(&request).expect("discovers");
        assert_eq!(response.schema_version, request.schema_version);
        assert_eq!(configured.toolchain_probe().label, "cargo");
        assert_eq!(
            configured.run_strategy_default(TaskKind::Build),
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

        // Exercise the RawOutputSink port (both live and block paths).
        let chunk = UnitOutput {
            unit_id: "rust:fake#test".into(),
            stream: OutputStream::Stdout,
            bytes: b"out".to_vec(),
        };
        raw_sink.live(&chunk).expect("live without error");
        raw_sink
            .block("rust:fake#test", std::slice::from_ref(&chunk))
            .expect("block without error");

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

        // Exercise the injected IO ports (toolchain / source-digest / cache).
        assert_eq!(
            prober
                .probe(
                    &ToolchainProbe::new("cargo", "cargo", vec!["--version".into()]),
                    Path::new("."),
                )
                .expect("probes"),
            "v1"
        );
        assert_eq!(
            digest.module(&module).expect("module digest"),
            format!("module:{}", module.id)
        );
        assert_eq!(
            digest.path(Path::new("shared")).expect("path digest"),
            "path:shared"
        );
        assert!(!cache.contains("any-key").expect("cache lookup"));

        // Exercise the APPLY-side ports (cache writer, command runner, held
        // process) enough to prove object-safety without spawning a runtime.
        cache_writer.record("any-key").expect("records");
        assert_eq!(held.unit_id(), "rust:fake#run");
        held.shutdown().expect("shuts down");
        let _runner: &dyn CommandRunner = &*runner;
    }

    #[test]
    fn watch_source_is_object_safe() {
        let watch: Box<dyn WatchSource> = Box::new(FakeWatchSource);
        let root = AbsPath::new(std::env::current_dir().expect("cwd")).expect("absolute");
        let _stream = watch
            .changes(
                std::slice::from_ref(&root),
                std::time::Duration::from_millis(200),
                tokio_util::sync::CancellationToken::new(),
            )
            .expect("watch stream");
    }
}
