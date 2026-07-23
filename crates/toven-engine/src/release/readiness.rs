//! Read-only `release readiness` projection: a fail-closed release preflight.
//!
//! Composes the recognized checks named in each releasable module's resolved
//! `[…release].readiness` list over the release scope and reports a single
//! go/no-go verdict with per-check detail. Any failing check makes the verdict
//! no-go — the gate fails closed. An unrecognized check name is a typed error
//! rather than a silent pass, so readiness can never be certified against a
//! check it did not evaluate.

use rskit_errors::{AppError, AppResult};
use toven_ports::{Provider, Reporter};

use super::plan::{release_targets, resolve_release_settings};
use crate::config::Document;
use crate::federation::baseline::MemberVcsReaders;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

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

/// Run the composed readiness checks over the release scope, fail-closed.
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
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let settings = resolve_release_settings(&context, &targets)?;

    let mut checks = Vec::new();
    for name in composed_check_names(&settings) {
        checks.push(evaluate_check(
            &name, &context, &targets, &settings, readers,
        )?);
    }
    Ok(ReadinessReport::new(checks))
}

/// Union the per-module readiness lists into a stable, first-seen-ordered set.
fn composed_check_names(
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
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

/// Evaluate one recognized check over the release scope.
fn evaluate_check(
    name: &str,
    context: &crate::plan::PlanContext,
    targets: &super::ReleaseTargets,
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
    readers: &MemberVcsReaders<'_>,
) -> AppResult<ReadinessCheck> {
    match name {
        CHECK_CLEAN_TREE => check_clean_tree(readers),
        CHECK_REGISTRY_IDEMPOTENT => check_registry_idempotent(context, targets, settings),
        other => Err(AppError::invalid_input(
            "release.readiness",
            format!(
                "unrecognized readiness check '{other}'; recognized checks are: {}",
                RECOGNIZED_CHECKS.join(", ")
            ),
        )),
    }
}

/// `clean-tree`: pass iff every member working tree has no uncommitted changes.
fn check_clean_tree(readers: &MemberVcsReaders<'_>) -> AppResult<ReadinessCheck> {
    let mut dirty = 0_usize;
    for entry in readers.entries() {
        dirty += entry.reader().worktree_status()?.len();
    }
    Ok(if dirty == 0 {
        ReadinessCheck::pass(CHECK_CLEAN_TREE, "working tree is clean")
    } else {
        ReadinessCheck::fail(
            CHECK_CLEAN_TREE,
            format!("{dirty} uncommitted change(s) in the working tree"),
        )
    })
}

/// `registry-idempotent`: pass iff no releasable module declares a version
/// strictly behind its highest published version — a regression that would
/// re-release an older version.
fn check_registry_idempotent(
    context: &crate::plan::PlanContext,
    targets: &super::ReleaseTargets,
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
) -> AppResult<ReadinessCheck> {
    let mut behind = Vec::new();
    for module in &context.federation.modules {
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        if !resolved.publication.publishes_to_registry() {
            continue;
        }
        let key = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&key) else {
            continue;
        };
        let declared = target.declared_version(module)?;
        if let Some(max_published) = target.published_versions(module)?.into_iter().max()
            && declared < max_published
        {
            behind.push(format!(
                "{} declares {declared}, behind published {max_published}",
                module.key()
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

    use super::release_readiness;
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::federation::baseline::MemberVcsReaders;
    use crate::plan::PlanRequest;

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
}
