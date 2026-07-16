//! Release PLAN tail orchestration.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_model::ModuleKey;
use toven_ports::{Provider, Reporter};

use crate::config::Document;
use crate::federation::baseline::MemberVcsReaders;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanContext, PlanRequest, prepare_front};

use super::{ReleasePlan, ReleaseStrategyName, ResolvedReleaseSettings, bump, change, changelog};

/// Build an immutable release plan.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, VCS failures, release
/// target failures, or invalid release strategy selection.
pub fn release_plan(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
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
    plan_with_context(&context, document, request, readers, &targets)
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
    context: &PlanContext,
    document: &Document,
    request: &PlanRequest,
    readers: &MemberVcsReaders<'_>,
    targets: &super::ReleaseTargets,
) -> AppResult<ReleasePlan> {
    let changes = change::detect(context, &request.selection, readers)?;
    plan_with_changes(context, document, request, &changes, targets)
}

fn plan_with_changes(
    context: &PlanContext,
    document: &Document,
    _request: &PlanRequest,
    changes: &change::ReleaseChanges,
    targets: &super::ReleaseTargets,
) -> AppResult<ReleasePlan> {
    let settings = resolve_release_settings(context, document, targets)?;
    let strategy = reconcile_strategy(&settings)?;
    let changelogs = context
        .federation
        .modules
        .iter()
        .map(|module| {
            let records = changes
                .records
                .get(&module.key())
                .cloned()
                .unwrap_or_default();
            (module.key(), changelog::entry(module, &records))
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
pub(crate) fn release_targets(context: &PlanContext) -> AppResult<super::ReleaseTargets> {
    let mut targets = super::ReleaseTargets::new();
    for (member, ecosystem, adapter) in context.adapters.iter() {
        if let Some(target) = adapter.release_target()? {
            targets.insert((member.cloned(), ecosystem.clone()), target);
        }
    }
    Ok(targets)
}

/// Fold each **releaseable** module's ecosystem-default and per-module release
/// override into its [`ResolvedReleaseSettings`].
///
/// Only modules with a release target participate: a non-publishable module
/// (e.g. `publish = false`) never joins a release plan, so its config must not
/// force a plan-wide strategy conflict. The ecosystem-level release config is
/// validated once per configured adapter; the per-module override (validated
/// structurally at load) is folded on top with the documented precedence
/// (`[modules.<name>.release]` > `[ecosystems.<id>].release` > adapter default).
///
/// # Errors
/// Propagates an invalid ecosystem release config or an unknown release strategy.
fn resolve_release_settings(
    context: &PlanContext,
    document: &Document,
    targets: &super::ReleaseTargets,
) -> AppResult<BTreeMap<ModuleKey, ResolvedReleaseSettings>> {
    for (_, ecosystem, adapter) in context.adapters.iter() {
        adapter
            .common()
            .release
            .validate(&format!("ecosystems.{ecosystem}.release"))?;
    }
    let mut resolved = BTreeMap::new();
    for module in &context.federation.modules {
        if !targets.contains_key(&(module.member.clone(), module.id.ecosystem.clone())) {
            continue;
        }
        let ecosystem = context
            .adapters
            .get(module.member.as_ref(), &module.id.ecosystem)
            .map(|adapter| adapter.common().release.clone())
            .unwrap_or_default();
        let over = document
            .modules
            .get(&module.id.to_string())
            .map(|entry| &entry.release);
        resolved.insert(
            module.key(),
            ResolvedReleaseSettings::resolve(&ecosystem, over)?,
        );
    }
    Ok(resolved)
}

/// Reconcile the single plan-wide bump strategy from per-module resolved
/// settings.
///
/// The engine produces one [`ReleasePlan`] with a single strategy, so every
/// module must resolve the same strategy. A conflict is a typed configuration
/// error rather than a silent first-wins pick; an empty release scope defaults to
/// [`ReleaseStrategyName::SemverCascade`].
fn reconcile_strategy(
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<ReleaseStrategyName> {
    let mut selected: Option<(ModuleKey, ReleaseStrategyName)> = None;
    for (module, resolved) in settings {
        match &selected {
            Some((existing_module, existing)) if *existing != resolved.strategy => {
                return Err(AppError::invalid_input(
                    "release.strategy",
                    format!(
                        "conflicting release strategies '{}' ({existing_module}) and '{}' ({module})",
                        existing.as_str(),
                        resolved.strategy.as_str()
                    ),
                ));
            }
            _ => selected = Some((module.clone(), resolved.strategy)),
        }
    }
    Ok(selected.map_or(ReleaseStrategyName::SemverCascade, |(_, strategy)| strategy))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, CommonEcosystemConfig, DiscoverResponse,
        Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::{ReleaseStrategyName, ResolvedReleaseSettings, reconcile_strategy, release_plan};
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::federation::baseline::MemberVcsReaders;
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
            modules: std::collections::BTreeMap::new(),
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
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("test"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let mut reporter = RecordingReporter::new();

        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let plan =
            release_plan(&request, &document(), &providers, &readers, &mut reporter).unwrap();

        assert_eq!(plan.publish_count(), 2);
        assert_eq!(plan.entries[0].module, core.key());
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
        assert_eq!(plan.entries[1].module, app.key());
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
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("test"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
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

        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let plan =
            release_plan(&request, &document(), &providers, &readers, &mut reporter).unwrap();
        let by_module = plan
            .entries
            .iter()
            .map(|entry| (entry.module.clone(), entry.changelog.lines.clone()))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            by_module.get(&core.key()).unwrap(),
            &vec!["crates/core/src/lib.rs".to_string()]
        );
        assert_eq!(
            by_module.get(&app.key()).unwrap(),
            &vec![
                "crates/app/src/lib.rs".to_string(),
                "crates/app/src/main.rs".to_string()
            ]
        );
    }

    #[test]
    fn release_plan_honors_the_configured_bump_strategy() {
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let mut common = CommonEcosystemConfig::default();
        common.release.strategy = Some("caret-prerelease".to_string());
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("test"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let mut reporter = RecordingReporter::new();

        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let plan =
            release_plan(&request, &document(), &providers, &readers, &mut reporter).unwrap();

        assert_eq!(plan.strategy, ReleaseStrategyName::CaretPrerelease);
    }

    fn settings_with_strategy(strategy: &str) -> ResolvedReleaseSettings {
        let config = ReleaseConfig {
            strategy: Some(strategy.to_string()),
            ..ReleaseConfig::default()
        };
        ResolvedReleaseSettings::resolve(&config, None).unwrap()
    }

    #[test]
    fn reconcile_strategy_defaults_to_semver_cascade_when_no_modules() {
        let empty = BTreeMap::new();
        assert_eq!(
            reconcile_strategy(&empty).unwrap(),
            ReleaseStrategyName::SemverCascade
        );
    }

    #[test]
    fn reconcile_strategy_rejects_conflicting_modules() {
        let mut settings = BTreeMap::new();
        settings.insert(
            module("core", "crates/core").key(),
            settings_with_strategy("semver-cascade"),
        );
        settings.insert(
            module("app", "crates/app").key(),
            settings_with_strategy("caret-prerelease"),
        );

        let error =
            reconcile_strategy(&settings).expect_err("conflicting strategies must be rejected");
        assert!(error.to_string().contains("conflicting release strategies"));
    }
}
