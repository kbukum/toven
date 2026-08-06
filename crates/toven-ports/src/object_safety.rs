//! Compile-time proof that every port trait is object-safe, with a trivial fake
//! impl per trait so the engine can store them as trait objects.

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
impl VersionSource for FakeReleaseTarget {
    fn declared_version(&self, _module: &Module) -> AppResult<Version> {
        Ok(Version::new(0, 1, 0))
    }
    fn published_versions(&self, _module: &Module) -> AppResult<Vec<Version>> {
        Ok(Vec::new())
    }
}
impl TagGrammar for FakeReleaseTarget {
    fn tag_scheme(&self, _module: &Module, _tag_format: Option<&str>) -> AppResult<TagScheme> {
        Ok(TagScheme::new("v", ""))
    }
}
impl Packager for FakeReleaseTarget {
    fn package(&self, _module: &Module) -> AppResult<Artifact> {
        Ok(Artifact::new("dist/fake.crate"))
    }
}
impl ManifestMutator for FakeReleaseTarget {
    fn apply_release(
        &self,
        _module: &Module,
        _mutation: &ReleaseMutation,
    ) -> AppResult<Vec<RepoPath>> {
        Ok(Vec::new())
    }
}
impl Publisher for FakeReleaseTarget {
    fn publish(
        &self,
        _module: &Module,
        _artifact: &Artifact,
        _credentials: &ReleaseCredentials,
        _visibility: Visibility,
    ) -> AppResult<PublishOutcome> {
        Ok(PublishOutcome::Published)
    }
}
impl SbomProducer for FakeReleaseTarget {}

struct FakeDelegatedPhase;
impl DelegatedPhase for FakeDelegatedPhase {
    fn run(&self, _request: &DelegatedPhaseRequest) -> AppResult<DelegatedPhaseOutcome> {
        Ok(DelegatedPhaseOutcome {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

struct FakeImagePhase;
impl ImagePhase for FakeImagePhase {
    fn publish_image(
        &self,
        _root: &Path,
        request: &ImageRequest,
    ) -> AppResult<ImagePublishOutcome> {
        Ok(ImagePublishOutcome::new(
            ImageOutcome::Pushed,
            "sha256:deadbeef",
            request.registries.clone(),
            request.sign,
        ))
    }
    fn resolve_digest(&self, _root: &Path, _reference: &str) -> AppResult<Option<String>> {
        Ok(None)
    }
}

struct FakeProvenancePhase;
impl ProvenancePhase for FakeProvenancePhase {
    fn attest(
        &self,
        _root: &Path,
        _subjects: &[ProvenanceSubject],
    ) -> AppResult<ProvenanceOutcome> {
        Ok(ProvenanceOutcome::Attested)
    }
    fn attestation_exists(&self, _root: &Path, _subject: &ProvenanceSubject) -> AppResult<bool> {
        Ok(false)
    }
}

struct FakeSigner;
impl Signer for FakeSigner {
    fn sign_blob(
        &self,
        _blob: &Path,
        _signature: &Path,
        _certificate: &Path,
        _signer: Option<&str>,
    ) -> AppResult<()> {
        Ok(())
    }
}

struct FakeDownloader;
impl AssetDownloader for FakeDownloader {
    fn download(&self, _tag: &str, _assets: &[&str], _dest: &Path) -> AppResult<()> {
        Ok(())
    }
}

struct FakeHookRunner;
impl HookRunner for FakeHookRunner {
    fn run_hook(&self, _phase: HookPhase, _reference: &str) -> AppResult<()> {
        Ok(())
    }
}

struct FakeVerifier;
impl SignatureVerifier for FakeVerifier {
    fn verify_blob(
        &self,
        _blob: &Path,
        _signature: &Path,
        _certificate: &Path,
        _identity: &str,
        _issuer: &str,
    ) -> AppResult<()> {
        Ok(())
    }
}

struct FakeVersionProbe;
impl VersionProbe for FakeVersionProbe {
    fn report_version(&self, _binary: &Path) -> AppResult<String> {
        Ok(String::new())
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
    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseAdapter>>> {
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
    fn current_branch(&self) -> AppResult<String> {
        Ok("main".to_string())
    }

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
    fn commits_since(
        &self,
        _since: Option<&str>,
        _path_prefix: Option<&Path>,
    ) -> AppResult<Vec<CommitSummary>> {
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
    fn commit(&self, _message: &str, _paths: &[&str]) -> AppResult<Oid> {
        Ok(Oid::new("deadbeef"))
    }
    fn stage(&self, _paths: &[&str]) -> AppResult<()> {
        Ok(())
    }
    fn preflight_tag_signer(&self, _signer: &TagSigner) -> AppResult<()> {
        Ok(())
    }
    fn create_tag(
        &self,
        _name: &str,
        _target_rev: &str,
        _message: Option<&str>,
        _signer: Option<&TagSigner>,
    ) -> AppResult<()> {
        Ok(())
    }
    fn push(&self, _remote: &str, _refspecs: &[String]) -> AppResult<()> {
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
    let mut raw_sink: Box<dyn RawOutputSink> = Box::new(FakeRawOutputSink { live: 0, blocks: 0 });
    let release: Box<dyn ReleaseAdapter> = Box::new(FakeReleaseTarget);
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

    // Exercise every release phase contract (directly and via the adapter seam).
    let target = configured.release_target().expect("ok").expect("present");
    assert_eq!(target.declared_version(&module).expect("ok").minor, 1);
    assert!(target.published_versions(&module).expect("ok").is_empty());
    let artifact = target.package(&module).expect("packages");
    target
        .apply_release(&module, &ReleaseMutation::version(Version::new(1, 0, 0)))
        .expect("applies");
    assert_eq!(
        target
            .publish(
                &module,
                &artifact,
                &ReleaseCredentials::default(),
                Visibility::Public
            )
            .expect("publishes"),
        PublishOutcome::Published
    );
    let direct_artifact = release.package(&module).expect("packages");
    assert_eq!(direct_artifact.path, artifact.path);

    // Exercise the DelegatedPhase port.
    let delegated: Box<dyn DelegatedPhase> = Box::new(FakeDelegatedPhase);
    let outcome = delegated
        .run(&DelegatedPhaseRequest::new(
            toven_model::ReleasePhase::Package,
            vec!["goreleaser".into(), "release".into()],
            DelegatedPhaseMode::Preview,
            "/repo",
        ))
        .expect("runs delegated phase");
    assert!(outcome.succeeded());

    // Exercise the Signer port.
    let signer: Box<dyn Signer> = Box::new(FakeSigner);
    signer
        .sign_blob(
            Path::new("dist/SHA256SUMS"),
            Path::new("dist/SHA256SUMS.sig"),
            Path::new("dist/SHA256SUMS.pem"),
            None,
        )
        .expect("signs without error");

    // Exercise the ImagePhase port.
    let image: Box<dyn ImagePhase> = Box::new(FakeImagePhase);
    let image_outcome = image
        .publish_image(
            Path::new("/repo"),
            &ImageRequest::new("services/api", "toven", "1.0.0")
                .with_registries(vec!["ghcr.io/acme".into()]),
        )
        .expect("publishes image");
    assert_eq!(image_outcome.outcome, ImageOutcome::Pushed);
    assert!(
        image
            .resolve_digest(Path::new("/repo"), "ghcr.io/acme/toven:1.0.0")
            .expect("resolves digest")
            .is_none()
    );

    // Exercise the ProvenancePhase port.
    let provenance: Box<dyn ProvenancePhase> = Box::new(FakeProvenancePhase);
    let subject = ProvenanceSubject::new("toven-x86_64.tar.gz", "sha256:abc");
    assert_eq!(
        provenance
            .attest(Path::new("/repo"), std::slice::from_ref(&subject))
            .expect("attests"),
        ProvenanceOutcome::Attested
    );
    assert!(
        !provenance
            .attestation_exists(Path::new("/repo"), &subject)
            .expect("checks attestation")
    );

    // Exercise the HookRunner port.
    let hook_runner: Box<dyn HookRunner> = Box::new(FakeHookRunner);
    hook_runner
        .run_hook(HookPhase::Pre, "test")
        .expect("runs hook without error");
    assert_eq!(HookPhase::Post.as_str(), "post");

    // Exercise the release-verification ports.
    let downloader: Box<dyn AssetDownloader> = Box::new(FakeDownloader);
    downloader
        .download("v1.0.0", &["SHA256SUMS"], Path::new("dist"))
        .expect("downloads without error");
    let verifier: Box<dyn SignatureVerifier> = Box::new(FakeVerifier);
    verifier
        .verify_blob(
            Path::new("dist/SHA256SUMS"),
            Path::new("dist/SHA256SUMS.sig"),
            Path::new("dist/SHA256SUMS.pem"),
            "identity",
            "issuer",
        )
        .expect("verifies without error");
    let probe: Box<dyn VersionProbe> = Box::new(FakeVersionProbe);
    probe
        .report_version(Path::new("dist/toven"))
        .expect("probes without error");

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
    assert_eq!(reader.current_branch().expect("branch"), "main");
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
    assert_eq!(
        writer.commit("msg", &["a.rs"]).expect("ok").as_str(),
        "deadbeef"
    );
    writer.stage(&["a.rs"]).expect("stages");
    writer
        .create_tag("v1", "HEAD", Some("msg"), None)
        .expect("tags");
    writer
        .push("origin", &["refs/heads/main".into()])
        .expect("pushes");
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

    // Exercise the APPLY-side ports (cache writer, command runner, held process)
    // enough to prove object-safety without spawning a runtime.
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
