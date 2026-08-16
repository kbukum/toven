//! Read-only `release readiness` projection: a fail-closed release preflight.
//!
//! Composes the recognized checks named in each releasable module's resolved
//! `[…release].readiness` list over the release scope and reports a single
//! go/no-go verdict with per-check detail. Any failing check makes the verdict
//! no-go — the gate fails closed. An unrecognized check name is a typed error
//! rather than a silent pass, so readiness can never be certified against a
//! check it did not evaluate.
//!
//! The verb runs on the shared runtime engine in the canonical
//! GATHER → per-unit STREAM shape: [`ReadinessInputs::gather`] resolves the
//! workspace prerequisites (release targets, resolved settings, the composed
//! check set, and — since the borrowed VCS readers cannot cross the worker pool
//! — a worktree-clean snapshot) exactly once, and [`ReadinessOperation`] streams
//! one composed check's verdict per unit. The checks are independent, so the
//! engine runs them bounded-parallel and each verdict settles live — the slow
//! `registry-idempotent` registry lookups no longer block the fast `clean-tree`
//! evaluation. [`release_readiness`] retains the buffered aggregate for
//! programmatic callers, assembled from the same per-check [`evaluate_check`]
//! compute.

use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use tokio_util::sync::CancellationToken;
use toven_model::{EcosystemId, MemberId, Module, ModuleKey};
use toven_ports::{Provider, Reporter};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use crate::ReleaseTargets;
use crate::planning::plan::{release_targets, resolve_release_settings};
use toven_core::config::Document;
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

/// Recognized check: every member working tree is clean.
const CHECK_CLEAN_TREE: &str = "clean-tree";
/// Recognized check: no releasable module declares a version behind the
/// registry.
const CHECK_REGISTRY_IDEMPOTENT: &str = "registry-idempotent";
/// Every recognized readiness check, for the actionable unknown-check error.
const RECOGNIZED_CHECKS: [&str; 2] = [CHECK_CLEAN_TREE, CHECK_REGISTRY_IDEMPOTENT];

/// One readiness check's verdict.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReadinessCheck {
    /// The recognized check name (e.g. `clean-tree`).
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Human-readable detail for the reporter.
    pub detail: String,
}

impl ReadinessCheck {
    /// A passing check with detail.
    #[must_use]
    pub fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: true,
            detail: detail.into(),
        }
    }

    /// A failing check with detail.
    #[must_use]
    pub fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: false,
            detail: detail.into(),
        }
    }
}

/// The aggregated release readiness report and its go/no-go verdict.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReadinessReport {
    /// Per-check verdicts in the order the checks were composed.
    pub checks: Vec<ReadinessCheck>,
}

impl ReadinessReport {
    /// Construct a readiness report.
    #[must_use]
    pub const fn new(checks: Vec<ReadinessCheck>) -> Self {
        Self { checks }
    }

    /// Whether the release is a go: every composed check passed. Fails closed —
    /// any failing check makes this `false`.
    #[must_use]
    pub fn is_go(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }
}

/// A releasable, registry-publishing module carried into the streamed
/// `registry-idempotent` check — fully owned so the per-unit phase borrows
/// nothing from the providers or VCS readers.
struct RegistryModule {
    /// The module's canonical key (for behind-registry detail).
    key: ModuleKey,
    /// The discovered module (drives the declared/published version reads).
    module: Module,
    /// The train the module's release target is keyed under.
    member: Option<MemberId>,
    /// The train's ecosystem.
    ecosystem: EcosystemId,
}

/// The shared, workspace-coupled prerequisites for `release readiness`, resolved
/// once by [`ReadinessInputs::gather`] and handed to every per-check unit.
pub struct ReadinessInputs {
    /// The composed check names (first-seen order across releasable modules).
    check_names: Vec<String>,
    /// Whether `clean-tree` is composed and, if so, the worktree-dirty count
    /// snapshotted during GATHER (the borrowed readers cannot cross the pool).
    clean_tree: Option<usize>,
    /// Release targets keyed by `(member, ecosystem)` — thread-safe so they can
    /// be shared across the engine's worker pool for the streamed registry I/O.
    targets: ReleaseTargets,
    /// The releasable, registry-publishing modules for `registry-idempotent`.
    registry_modules: Vec<RegistryModule>,
}

impl ReadinessInputs {
    /// Resolve the release targets, settings, composed check set, and (when
    /// `clean-tree` is composed) the worktree-clean snapshot once.
    ///
    /// # Errors
    /// Propagates configuration/discovery/graph failures and VCS worktree-status
    /// failures (the latter only when `clean-tree` is composed).
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
        readers: &MemberVcsReaders<'_>,
        reporter: &mut dyn Reporter,
    ) -> AppResult<Self> {
        let locator = PathDriverLocator::new();
        let context = prepare_front(
            &request.project_root,
            document,
            providers,
            &locator,
            reporter,
        )?;
        let targets = release_targets(&context, readers)?;
        let settings = resolve_release_settings(&context, &targets)?;
        let check_names = composed_check_names(&settings);

        // The borrowed VCS readers cannot cross the worker pool, so the
        // worktree-clean state is snapshotted here rather than in the unit —
        // fast git I/O, only when the check is actually composed.
        let clean_tree = if check_names.iter().any(|name| name == CHECK_CLEAN_TREE) {
            let mut dirty = 0_usize;
            for entry in readers.entries() {
                dirty += entry.reader().worktree_status()?.len();
            }
            Some(dirty)
        } else {
            None
        };

        let mut registry_modules = Vec::new();
        for module in &context.federation.modules {
            let Some(resolved) = settings.get(&module.key()) else {
                continue;
            };
            if !resolved.publication.publishes_to_registry() {
                continue;
            }
            let key = (module.member.clone(), module.id.ecosystem.clone());
            if !targets.contains_key(&key) {
                continue;
            }
            registry_modules.push(RegistryModule {
                key: module.key(),
                module: module.clone(),
                member: module.member.clone(),
                ecosystem: module.id.ecosystem.clone(),
            });
        }

        Ok(Self {
            check_names,
            clean_tree,
            targets,
            registry_modules,
        })
    }

    /// The engine unit graph: one independent (edgeless) unit per composed
    /// check, so the engine schedules them as a single bounded-parallel wave.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        self.check_names
            .iter()
            .map(|name| UnitSpec::new(name.clone(), Vec::<String>::new()))
            .collect()
    }
}

/// Evaluate one composed check over the gathered inputs.
///
/// Pure over [`ReadinessInputs`]: `clean-tree` reads the gathered snapshot;
/// `registry-idempotent` performs the per-module registry I/O the engine streams
/// per unit. An unrecognized name fails closed with a typed error so readiness
/// can never certify against a check it did not evaluate.
///
/// # Errors
/// Returns [`ErrorCode::Internal`] if `clean-tree` was composed without its
/// gathered snapshot, propagates release-target version I/O failures, and
/// returns an invalid-input error for an unrecognized check name.
fn evaluate_check(inputs: &ReadinessInputs, name: &str) -> AppResult<ReadinessCheck> {
    match name {
        CHECK_CLEAN_TREE => {
            let dirty = inputs.clean_tree.ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    "clean-tree was composed without its gathered worktree snapshot",
                )
            })?;
            Ok(eval_clean_tree(dirty))
        }
        CHECK_REGISTRY_IDEMPOTENT => eval_registry_idempotent(inputs),
        other => Err(AppError::invalid_input(
            "release.readiness",
            format!(
                "unrecognized readiness check '{other}'; recognized checks are: {}",
                RECOGNIZED_CHECKS.join(", ")
            ),
        )),
    }
}

/// `clean-tree`: pass iff the gathered worktree-dirty count is zero.
fn eval_clean_tree(dirty: usize) -> ReadinessCheck {
    if dirty == 0 {
        ReadinessCheck::pass(CHECK_CLEAN_TREE, "working tree is clean")
    } else {
        ReadinessCheck::fail(
            CHECK_CLEAN_TREE,
            format!("{dirty} uncommitted change(s) in the working tree"),
        )
    }
}

/// `registry-idempotent`: pass iff no releasable module declares a version
/// strictly behind its highest published version — a regression that would
/// re-release an older version.
///
/// # Errors
/// Returns [`ErrorCode::Internal`] if a gathered module's release target is
/// missing, and propagates release-target version I/O failures.
fn eval_registry_idempotent(inputs: &ReadinessInputs) -> AppResult<ReadinessCheck> {
    let mut behind = Vec::new();
    for module in &inputs.registry_modules {
        let target = inputs
            .targets
            .get(&(module.member.clone(), module.ecosystem.clone()))
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!("module '{}' has no gathered release target", module.key),
                )
            })?;
        let declared = target.declared_version(&module.module)?;
        if let Some(max_published) = target.published_versions(&module.module)?.into_iter().max()
            && declared < max_published
        {
            behind.push(format!(
                "{} declares {declared}, behind published {max_published}",
                module.key
            ));
        }
    }
    Ok(if behind.is_empty() {
        ReadinessCheck::pass(
            CHECK_REGISTRY_IDEMPOTENT,
            "no module declares a version behind the registry",
        )
    } else {
        ReadinessCheck::fail(CHECK_REGISTRY_IDEMPOTENT, behind.join("; "))
    })
}

/// The `release readiness` per-unit operation on the shared runtime engine.
///
/// GATHER (targets, settings, the composed check set, and the worktree-clean
/// snapshot) is resolved once into [`ReadinessInputs`]; each unit streams one
/// composed check's verdict. The `registry-idempotent` check's registry lookups
/// are synchronous port calls, so the unit runs on a blocking thread
/// ([`tokio::task::spawn_blocking`]) to let the async engine schedule the checks
/// bounded-parallel.
pub struct ReadinessOperation {
    inputs: Arc<ReadinessInputs>,
}

impl ReadinessOperation {
    /// Wrap gathered inputs as a runnable operation.
    #[must_use]
    pub fn new(inputs: ReadinessInputs) -> Self {
        Self {
            inputs: Arc::new(inputs),
        }
    }
}

#[async_trait]
impl UnitOperation for ReadinessOperation {
    type Shared = Arc<ReadinessInputs>;
    type Outcome = ReadinessCheck;

    async fn gather(&self) -> AppResult<Self::Shared> {
        Ok(Arc::clone(&self.inputs))
    }

    async fn run(
        &self,
        shared: &Self::Shared,
        unit_id: &str,
        _cancel: CancellationToken,
    ) -> AppResult<Completed<Self::Outcome>> {
        let shared = Arc::clone(shared);
        let name = unit_id.to_string();
        let check = tokio::task::spawn_blocking(move || evaluate_check(&shared, &name))
            .await
            .map_err(AppError::internal)??;
        // A failing check is a successful unit carrying a `passed = false`
        // verdict, not a unit error; only I/O or an unknown check errors the
        // unit. The go/no-go verdict is assembled from the streamed outcomes.
        Ok(Completed::succeeded(check))
    }
}

/// Build the `release readiness` operation and its engine unit graph.
///
/// The single entry the CLI drives on [`toven_runtime::execute`]: GATHER runs
/// here (once), and the returned units feed the engine's per-check streaming.
///
/// # Errors
/// Propagates GATHER failures (configuration/discovery/graph, VCS worktree
/// status when `clean-tree` is composed).
pub fn readiness_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<(ReadinessOperation, Vec<UnitSpec>)> {
    let inputs = ReadinessInputs::gather(request, document, providers, readers, reporter)?;
    let units = inputs.units();
    Ok((ReadinessOperation::new(inputs), units))
}

/// Run the composed readiness checks over the release scope, fail-closed, as a
/// buffered aggregate.
///
/// Retained for programmatic callers; assembled from the same per-check
/// `evaluate_check` compute the streaming [`ReadinessOperation`] drives, so the
/// two never diverge. The CLI streams via the engine instead of calling this.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, VCS/registry I/O
/// failures, and an unrecognized configured check name.
pub fn release_readiness(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<ReadinessReport> {
    let inputs = ReadinessInputs::gather(request, document, providers, readers, reporter)?;
    let checks = inputs
        .check_names
        .iter()
        .map(|name| evaluate_check(&inputs, name))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(ReadinessReport::new(checks))
}

/// Union the per-module readiness lists into a stable, first-seen-ordered set.
fn composed_check_names(
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, crate::ResolvedReleaseSettings>,
) -> Vec<String> {
    let mut names = Vec::new();
    for resolved in settings.values() {
        for check in &resolved.readiness {
            if !names.contains(check) {
                names.push(check.clone());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, CommonEcosystemConfig, DiscoverResponse,
        Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::{ReadinessCheck, readiness_operation, release_readiness};
    use rskit_errors::AppResult;
    use toven_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_core::federation::baseline::MemberVcsReaders;
    use toven_core::plan::PlanRequest;

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(eid("rust"), name).unwrap(),
            RepoPath::new(format!("crates/{name}")).unwrap(),
        )
    }

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid("rust"), RawValue::from(json!({ "release": {} })));
        Document {
            project: ProjectConfig {
                name: "demo".to_string(),
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
            units: std::collections::BTreeMap::new(),
        }
    }

    fn request() -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
    }

    fn providers_with(target: FakeReleaseTarget, checks: &[&str]) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let registry = checks
            .contains(&super::CHECK_REGISTRY_IDEMPOTENT)
            .then_some("crates-io".to_string());
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry,
                readiness: Some(checks.iter().map(ToString::to_string).collect()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    #[test]
    fn readiness_is_go_when_every_check_passes() {
        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 2, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let provider = providers_with(target, &["clean-tree", "registry-idempotent"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let report =
            release_readiness(&request(), &document(), &providers, &readers, &mut reporter)
                .unwrap();

        assert!(report.is_go());
        assert_eq!(report.checks.len(), 2);
    }

    #[test]
    fn readiness_fails_closed_when_the_tree_is_dirty() {
        let target = FakeReleaseTarget::new();
        let provider = providers_with(target, &["clean-tree"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new().with_worktree_status(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let report =
            release_readiness(&request(), &document(), &providers, &readers, &mut reporter)
                .unwrap();

        assert!(!report.is_go());
        assert_eq!(report.checks[0].name, "clean-tree");
        assert!(!report.checks[0].passed);
    }

    #[test]
    fn readiness_fails_when_a_module_is_behind_the_registry() {
        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 1, 0))
            .with_published_versions(vec![Version::new(0, 3, 0)]);
        let provider = providers_with(target, &["registry-idempotent"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let report =
            release_readiness(&request(), &document(), &providers, &readers, &mut reporter)
                .unwrap();

        assert!(!report.is_go());
        assert!(report.checks[0].detail.contains("behind"));
    }

    #[test]
    fn unrecognized_check_is_a_typed_error() {
        let provider = providers_with(FakeReleaseTarget::new(), &["nonsense-check"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let error = release_readiness(&request(), &document(), &providers, &readers, &mut reporter)
            .expect_err("an unknown check must fail closed with a typed error");
        assert!(error.to_string().contains("nonsense-check"));
    }

    #[test]
    fn empty_readiness_list_is_a_vacuous_go() {
        let provider = providers_with(FakeReleaseTarget::new(), &[]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let report =
            release_readiness(&request(), &document(), &providers, &readers, &mut reporter)
                .unwrap();

        assert!(report.is_go());
        assert!(report.checks.is_empty());
    }

    /// Records the streamed per-unit lifecycle in arrival order.
    #[derive(Default)]
    struct Recorder {
        started: Vec<String>,
        settled: Vec<(String, toven_runtime::UnitStatus, Option<ReadinessCheck>)>,
    }

    impl toven_runtime::Progress<ReadinessCheck> for Recorder {
        fn started(&mut self, unit_id: &str) -> AppResult<()> {
            self.started.push(unit_id.to_string());
            Ok(())
        }

        fn settled(&mut self, report: &toven_runtime::UnitReport<ReadinessCheck>) -> AppResult<()> {
            self.settled.push((
                report.unit_id.clone(),
                report.status,
                report.outcome.clone(),
            ));
            Ok(())
        }
    }

    #[test]
    fn readiness_units_are_edgeless_one_per_composed_check() {
        // The composed checks are independent, so every unit is edgeless — the
        // engine schedules them as one bounded-parallel wave.
        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 2, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let provider = providers_with(target, &["clean-tree", "registry-idempotent"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (_op, units) =
            readiness_operation(&request(), &document(), &providers, &readers, &mut reporter)
                .unwrap();

        assert_eq!(units.len(), 2);
        assert!(
            units.iter().all(|unit| unit.depends_on.is_empty()),
            "readiness units must be edgeless: {units:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readiness_streams_a_settled_verdict_per_composed_check() {
        // The engine streams a start + settled pair per check, each settled
        // event carrying that check's typed verdict — never buffering the full
        // report before first output. A failing check settles as a successful
        // unit whose verdict is `passed = false`.
        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 1, 0))
            .with_published_versions(vec![Version::new(0, 3, 0)]);
        let provider = providers_with(target, &["clean-tree", "registry-idempotent"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let vcs = FakeVcsReader::new().with_worktree_status(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (op, units) =
            readiness_operation(&request(), &document(), &providers, &readers, &mut reporter)
                .unwrap();
        let mut rec = Recorder::default();
        let summary = toven_runtime::execute(
            &units,
            op,
            toven_runtime::EngineConfig {
                jobs: 2,
                fail_fast: false,
            },
            &mut rec,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 2);
        assert!(!summary.has_failures());
        assert_eq!(rec.started.len(), 2);
        assert_eq!(rec.settled.len(), 2);
        // Both checks fail their verdict (dirty tree, behind registry), but each
        // still settles as a successful unit carrying the typed verdict.
        for (unit_id, status, outcome) in &rec.settled {
            assert_eq!(*status, toven_runtime::UnitStatus::Succeeded);
            let check = outcome.as_ref().expect("settled check carries a verdict");
            assert_eq!(&check.name, unit_id);
            assert!(!check.passed, "expected a failing verdict for {unit_id}");
        }
    }
}
