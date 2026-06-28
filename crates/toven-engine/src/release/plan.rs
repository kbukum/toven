//! Release PLAN tail orchestration.

use std::collections::BTreeMap;

use rskit_config::{RawValue, deserialize_subtree};
use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{Provider, ReleaseTarget, Reporter, VcsReader};

use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

use super::{ReleasePlan, bump, change, changelog, strategy};

/// Build an immutable release plan.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, VCS failures, release
/// target failures, or invalid release strategy selection.
pub fn release_plan(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    vcs: &dyn VcsReader,
    reporter: &mut dyn Reporter,
) -> AppResult<ReleasePlan> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    plan_with_context(&context, document, request, vcs, &targets)
}

/// Build a [`ReleasePlan`] from an already-prepared [`PlanContext`] and its
/// resolved release `targets`.
///
/// Shared by [`release_plan`] and the combined [`release_run`](super::release_run)
/// facade so the PLAN cut is computed by exactly one path.
///
/// # Errors
/// Propagates strategy selection, change-detection, and bump-planning failures.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn plan_with_context(
    context: &crate::plan::PlanContext,
    document: &Document,
    request: &PlanRequest,
    vcs: &dyn VcsReader,
    targets: &BTreeMap<EcosystemId, Box<dyn ReleaseTarget>>,
) -> AppResult<ReleasePlan> {
    let strategy = strategy::resolve(release_strategy(document)?.as_deref())?;
    let changes = change::detect(context, document, &request.selection, vcs)?;
    let changelogs = context
        .federation
        .modules
        .iter()
        .map(|module| {
            let records = changes.records.get(&module.id).cloned().unwrap_or_default();
            (module.id.clone(), changelog::entry(module, &records))
        })
        .collect::<BTreeMap<_, _>>();
    let entries = bump::plan_entries(&bump::BumpInputs {
        graph: &context.graph,
        modules: &context.federation.modules,
        edges: &context.federation.edges,
        changed: &changes.changed,
        baselines: &changes.baselines,
        changelogs: &changelogs,
        targets,
        release_strategy: strategy,
    })?;

    Ok(ReleasePlan::new(strategy, entries))
}

/// Resolve the release targets declared by each configured ecosystem adapter.
///
/// # Errors
/// Propagates a release target's construction failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn release_targets(
    context: &crate::plan::PlanContext,
) -> AppResult<BTreeMap<EcosystemId, Box<dyn ReleaseTarget>>> {
    let mut targets = BTreeMap::new();
    for (ecosystem, adapter) in &context.adapters {
        if let Some(target) = adapter.release_target()? {
            targets.insert(ecosystem.clone(), target);
        }
    }
    Ok(targets)
}

/// Resolve the single release strategy declared across ecosystem sections.
///
/// The engine produces one [`ReleasePlan`] with a single strategy, so every
/// ecosystem that names a `release.strategy` must agree. A conflict is a typed
/// configuration error rather than a silent first-wins pick.
fn release_strategy(document: &Document) -> AppResult<Option<String>> {
    let mut selected: Option<String> = None;
    for (ecosystem, raw) in &document.ecosystems {
        let Some(strategy) = strategy_of(ecosystem, raw)? else {
            continue;
        };
        match &selected {
            Some(existing) if existing != &strategy => {
                return Err(AppError::invalid_input(
                    "release.strategy",
                    format!(
                        "conflicting release strategies '{existing}' and '{strategy}' across ecosystems"
                    ),
                ));
            }
            _ => selected = Some(strategy),
        }
    }
    Ok(selected)
}

/// Partial view over one ecosystem section's `release.strategy`.
///
/// Permissive by design (no `deny_unknown_fields`): an ecosystem section carries
/// many adapter-owned keys the engine ignores here. But a malformed `release`
/// table or a non-string `strategy` now surfaces as a typed configuration error
/// instead of being silently treated as "no strategy declared".
#[derive(serde::Deserialize)]
struct EcosystemReleaseView {
    #[serde(default)]
    release: Option<ReleaseStrategyView>,
}

#[derive(serde::Deserialize)]
struct ReleaseStrategyView {
    #[serde(default)]
    strategy: Option<String>,
}

/// Extract `release.strategy` from one raw ecosystem section, if present.
fn strategy_of(ecosystem: &EcosystemId, raw: &RawValue) -> AppResult<Option<String>> {
    let view: EcosystemReleaseView = deserialize_subtree(ecosystem.as_str(), raw.clone())?;
    Ok(view.release.and_then(|release| release.strategy))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, DiscoverResponse, Provider, TaskKind,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::release_plan;
    use super::release_strategy;
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::plan::{PlanRequest, Selection};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(eid("rust"), name).unwrap()
    }

    fn module(name: &str, root: &str) -> Module {
        Module::new(mref(name), RepoPath::new(root).unwrap())
    }

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid("rust"), RawValue::from(json!({ "release": {} })));
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems,
            members: Vec::new(),
        }
    }

    #[test]
    fn release_plan_bumps_changed_module_and_dependent_floor() {
        let core = module("core", "crates/core");
        let app = module("app", "crates/app");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core.clone(), app.clone()];
        response.edges = vec![Edge::new(app.id.clone(), core.id.clone(), DepKind::Normal)];

        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new("r1", "t", TaskKind::Test, AbsPath::new("/repo").unwrap())
            .with_selection(Selection::Changed(BaselineSpec::explicit("main")));
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let mut reporter = RecordingReporter::new();

        let plan = release_plan(&request, &document(), &providers, &vcs, &mut reporter).unwrap();

        assert_eq!(plan.publish_count(), 2);
        assert_eq!(plan.entries[0].module, core.id);
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
        assert_eq!(plan.entries[1].module, app.id);
        assert_eq!(
            plan.entries[1]
                .mutation
                .dep_floor_updates
                .get(&mref("core")),
            Some(&Version::new(0, 1, 1))
        );
    }

    #[test]
    fn release_plan_changelogs_only_include_module_owned_records() {
        let core = module("core", "crates/core");
        let app = module("app", "crates/app");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core.clone(), app.clone()];

        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new("r1", "t", TaskKind::Test, AbsPath::new("/repo").unwrap())
            .with_selection(Selection::Changed(BaselineSpec::explicit("main")));
        let vcs = FakeVcsReader::new()
            .with_changed_since(vec![
                ChangeRecord::new("crates/core/src/lib.rs", ChangeStatus::Modified),
                ChangeRecord::new("crates/app/src/lib.rs", ChangeStatus::Modified),
            ])
            .with_worktree_status(vec![ChangeRecord::new(
                "crates/app/src/main.rs",
                ChangeStatus::Modified,
            )]);
        let mut reporter = RecordingReporter::new();

        let plan = release_plan(&request, &document(), &providers, &vcs, &mut reporter).unwrap();
        let by_module = plan
            .entries
            .iter()
            .map(|entry| (entry.module.clone(), entry.changelog.lines.clone()))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            by_module.get(&core.id).unwrap(),
            &vec!["crates/core/src/lib.rs".to_string()]
        );
        assert_eq!(
            by_module.get(&app.id).unwrap(),
            &vec![
                "crates/app/src/lib.rs".to_string(),
                "crates/app/src/main.rs".to_string()
            ]
        );
    }

    #[test]
    fn release_strategy_reads_a_single_declared_strategy() {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(
            eid("rust"),
            RawValue::from(json!({ "release": { "strategy": "caret-prerelease" } })),
        );
        ecosystems.insert(eid("go"), RawValue::from(json!({ "release": {} })));
        let mut doc = document();
        doc.ecosystems = ecosystems;

        assert_eq!(
            release_strategy(&doc).unwrap().as_deref(),
            Some("caret-prerelease")
        );
    }

    #[test]
    fn release_strategy_rejects_conflicting_declarations() {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(
            eid("rust"),
            RawValue::from(json!({ "release": { "strategy": "semver-cascade" } })),
        );
        ecosystems.insert(
            eid("go"),
            RawValue::from(json!({ "release": { "strategy": "caret-prerelease" } })),
        );
        let mut doc = document();
        doc.ecosystems = ecosystems;

        let error = release_strategy(&doc).expect_err("conflicting strategies must be rejected");
        assert!(error.to_string().contains("conflicting release strategies"));
    }

    #[test]
    fn release_strategy_rejects_a_malformed_release_section() {
        // A non-string `strategy` must surface a typed error rather than being
        // silently dropped (the previous `.ok()?` swallowed conversion failures).
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(
            eid("rust"),
            RawValue::from(json!({ "release": { "strategy": 7 } })),
        );
        let mut doc = document();
        doc.ecosystems = ecosystems;

        assert!(
            release_strategy(&doc).is_err(),
            "a non-string strategy must be a typed error, not a silent default"
        );
    }
}
