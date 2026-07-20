//! Release PLAN tail orchestration.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::ModuleKey;
use toven_ports::{Provider, Reporter};

use crate::config::Document;
use crate::federation::baseline::MemberVcsReaders;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanContext, PlanRequest, prepare_front};

use super::{
    BumpOverrides, BumpPolicy, ReleasePlan, ResolvedReleaseSettings, bump, change, changelog,
};

/// Build an immutable release plan.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, VCS failures, release
/// target failures, or invalid bump-policy selection.
pub fn release_plan(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    overrides: &BumpOverrides,
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
    plan_with_context(&context, request, readers, overrides, &targets)
}

/// Build a [`ReleasePlan`] from an already-prepared [`PlanContext`] and its
/// resolved release `targets`.
///
/// Shared by [`release_plan`] and the combined
/// [`release_run`](super::release_run) facade so the PLAN cut is computed by
/// exactly one path.
///
/// # Errors
/// Propagates bump-policy selection, change-detection, and bump-planning
/// failures.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn plan_with_context(
    context: &PlanContext,
    request: &PlanRequest,
    readers: &MemberVcsReaders<'_>,
    overrides: &BumpOverrides,
    targets: &super::ReleaseTargets,
) -> AppResult<ReleasePlan> {
    let settings = resolve_release_settings(context, targets)?;
    let changes = change::detect(
        context,
        &request.selection,
        overrides.base(),
        readers,
        targets,
        &settings,
    )?;
    validate_required_changelogs(&changes, &settings)?;
    plan_with_changes(context, request, &changes, overrides, targets, &settings)
}

fn plan_with_changes(
    context: &PlanContext,
    _request: &PlanRequest,
    changes: &change::ReleaseChanges,
    overrides: &BumpOverrides,
    targets: &super::ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<ReleasePlan> {
    let policy = reconcile_policy(settings)?;
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
        settings,
        targets,
        policy,
        overrides,
    })?;

    Ok(ReleasePlan::new(policy, entries))
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
/// Only modules whose `(member, ecosystem)` has a release target participate:
/// an ecosystem/member with no release target (e.g. a non-publishable adapter)
/// never joins a release plan, so its config must not force a plan-wide policy
/// conflict. The ecosystem-level release config is validated once per
/// configured adapter; the per-module override (validated structurally at load)
/// is folded on top with the documented precedence (`[modules.<name>.release]` >
/// `[ecosystems.<id>].release` > adapter default).
///
/// # Errors
/// Propagates an invalid ecosystem release config or an unknown bump policy.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn resolve_release_settings(
    context: &PlanContext,
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
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "module '{}' has a release target but no configured adapter",
                        module.id
                    ),
                )
            })?
            .common()
            .release
            .clone();
        let member_document = context
            .composed
            .members()
            .iter()
            .find(|member| member.member().id() == module.member.as_ref())
            .map(crate::federation::compose::ComposedMember::document)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "module '{}' has no composed member configuration",
                        module.key()
                    ),
                )
            })?;
        let over = member_document
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

/// Reconcile the single plan-wide bump policy from per-module resolved
/// settings.
///
/// The engine produces one [`ReleasePlan`] with a single policy, so every
/// module must resolve the same policy. A conflict is a typed configuration
/// error rather than a silent first-wins pick; an empty release scope defaults
/// to [`BumpPolicy::SemverCascade`].
fn reconcile_policy(
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<BumpPolicy> {
    let mut selected: Option<(ModuleKey, BumpPolicy)> = None;
    for (module, resolved) in settings {
        match &selected {
            Some((existing_module, existing)) if *existing != resolved.policy => {
                return Err(AppError::invalid_input(
                    "release.strategy",
                    format!(
                        "conflicting bump policies '{}' ({existing_module}) and '{}' ({module})",
                        existing.as_str(),
                        resolved.policy.as_str()
                    ),
                ));
            }
            _ => selected = Some((module.clone(), resolved.policy)),
        }
    }
    Ok(selected.map_or(BumpPolicy::SemverCascade, |(_, policy)| policy))
}

fn validate_required_changelogs(
    changes: &change::ReleaseChanges,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<()> {
    for module in &changes.changed {
        let Some(resolved) = settings.get(module) else {
            continue;
        };
        if resolved.changelog.required
            && changes
                .records
                .get(module)
                .is_none_or(std::vec::Vec::is_empty)
        {
            return Err(AppError::invalid_input(
                "release.changelog.required",
                format!("changed module '{module}' has no changelog entry"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{
        AbsPath, DepKind, EcosystemId, Edge, Module, ModuleKey, ModuleRef, RepoPath,
    };
    use toven_ports::{
        BaselineSpec, BumpLevel, ChangeRecord, ChangeStatus, ChangelogConfig,
        CommonEcosystemConfig, DependentVersion, DiscoverResponse, PrereleaseConfig, Provider,
        TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::{
        BumpPolicy, ResolvedReleaseSettings, reconcile_policy, release_plan,
        validate_required_changelogs,
    };
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::federation::baseline::MemberVcsReaders;
    use crate::plan::{PlanRequest, Selection};
    use crate::release::{BumpOverrides, BumpReason, BumpSource, ReleasePlan};

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

    /// Plan a single changed `core` module against the given `common` config
    /// and per-run `overrides`, with `target` supplying its declared/published
    /// state.
    fn plan_core(
        common: CommonEcosystemConfig,
        target: FakeReleaseTarget,
        overrides: &BumpOverrides,
    ) -> ReleasePlan {
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            overrides,
            &mut reporter,
        )
        .unwrap()
    }

    fn common_with_level(level: BumpLevel) -> CommonEcosystemConfig {
        let mut common = CommonEcosystemConfig::default();
        common.release.level = Some(level);
        common
    }

    #[test]
    fn required_changelog_rejects_changed_module_without_records() {
        let key = ModuleKey::bare(mref("core"));
        let mut changed_modules = std::collections::BTreeSet::new();
        changed_modules.insert(key.clone());
        let changes = crate::release::change::ReleaseChanges {
            changed: changed_modules,
            records: BTreeMap::new(),
            baselines: BTreeMap::new(),
        };
        let mut config = CommonEcosystemConfig::default();
        config.release.changelog = Some(ChangelogConfig {
            path: None,
            required: true,
        });
        let resolved = ResolvedReleaseSettings::resolve(&config.release, None).unwrap();
        let settings = BTreeMap::from([(key, resolved)]);

        let error = validate_required_changelogs(&changes, &settings)
            .expect_err("required changelog must reject missing records");

        assert!(error.to_string().contains("release.changelog.required"));
        assert!(error.to_string().contains("rust:core"));
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
        let overrides = BumpOverrides::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &overrides,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(plan.publish_count(), 2);
        assert_eq!(plan.entries[0].module, core.key());
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
        assert_eq!(plan.entries[0].reason, BumpReason::Changed);
        assert_eq!(plan.entries[0].winning_input, BumpSource::Default);
        assert_eq!(plan.entries[1].module, app.key());
        assert_eq!(plan.entries[1].reason, BumpReason::DependencyCascade);
        assert_eq!(plan.entries[1].cascade_origin, Some(core.key()));
        assert_eq!(
            plan.entries[1]
                .mutation
                .dep_floor_updates
                .get(&mref("core")),
            Some(&Version::new(0, 1, 1))
        );
    }

    #[test]
    fn release_plan_cascades_transitively_to_indirect_dependents() {
        let core = module("core", "crates/core");
        let mid = module("mid", "crates/mid");
        let top = module("top", "crates/top");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core.clone(), mid.clone(), top.clone()];
        response.edges = vec![
            Edge::new(mid.id.clone(), core.id.clone(), DepKind::Normal),
            Edge::new(top.id.clone(), mid.id, DepKind::Normal),
        ];

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
        let overrides = BumpOverrides::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &overrides,
            &mut reporter,
        )
        .unwrap();

        // core changed → mid (direct) and top (transitive) both cascade, and both
        // report the changed root as the cascade origin.
        assert_eq!(plan.publish_count(), 3);
        assert_eq!(plan.entries[2].module, top.key());
        assert_eq!(plan.entries[2].reason, BumpReason::DependencyCascade);
        assert_eq!(plan.entries[2].cascade_origin, Some(core.key()));
        assert_eq!(plan.entries[2].planned_version, Some(Version::new(0, 1, 1)));
        assert_eq!(
            plan.entries[2].mutation.dep_floor_updates.get(&mref("mid")),
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
        let overrides = BumpOverrides::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &overrides,
            &mut reporter,
        )
        .unwrap();
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
    fn config_minor_level_bumps_minor() {
        let plan = plan_core(
            common_with_level(BumpLevel::Minor),
            FakeReleaseTarget::new(),
            &BumpOverrides::new(),
        );
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 2, 0)));
        assert_eq!(plan.entries[0].level, BumpLevel::Minor);
        assert_eq!(plan.entries[0].winning_input, BumpSource::Config);
    }

    #[test]
    fn argv_level_override_beats_config() {
        let overrides = BumpOverrides::new()
            .with_module_level(mref("core"), BumpLevel::Major)
            .unwrap();
        let plan = plan_core(
            common_with_level(BumpLevel::Minor),
            FakeReleaseTarget::new(),
            &overrides,
        );
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(1, 0, 0)));
        assert_eq!(plan.entries[0].level, BumpLevel::Major);
        assert_eq!(plan.entries[0].winning_input, BumpSource::Argv);
    }

    #[test]
    fn set_version_pins_an_explicit_target() {
        let overrides = BumpOverrides::new()
            .with_set_version(mref("core"), Version::new(3, 1, 4))
            .unwrap();
        let plan = plan_core(
            CommonEcosystemConfig::default(),
            FakeReleaseTarget::new(),
            &overrides,
        );
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(3, 1, 4)));
        assert_eq!(plan.entries[0].reason, BumpReason::Explicit);
        assert_eq!(plan.entries[0].winning_input, BumpSource::SetVersion);
    }

    #[test]
    fn pre_channel_cuts_a_prerelease() {
        let mut common = CommonEcosystemConfig::default();
        common.release.prerelease = Some(PrereleaseConfig {
            channels: vec!["rc".to_string()],
            ..PrereleaseConfig::default()
        });
        let overrides = BumpOverrides::new().with_prerelease("rc");
        let plan = plan_core(common, FakeReleaseTarget::new(), &overrides);
        assert_eq!(
            plan.entries[0].planned_version,
            Some(Version::parse("0.1.1-rc.1").unwrap())
        );
        assert_eq!(plan.entries[0].prerelease_channel.as_deref(), Some("rc"));
    }

    #[test]
    fn unrecognized_pre_channel_is_rejected() {
        let overrides = BumpOverrides::new().with_prerelease("nightly");
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        assert!(
            release_plan(
                &request,
                &document(),
                &providers,
                &readers,
                &overrides,
                &mut reporter,
            )
            .is_err()
        );
    }

    #[test]
    fn module_at_registry_max_is_a_reported_no_op() {
        let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);
        let plan = plan_core(
            CommonEcosystemConfig::default(),
            target,
            &BumpOverrides::new(),
        );
        // Patch of 0.1.0 → 0.1.1 which the registry already has: up to date.
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
        assert!(plan.entries[0].up_to_date);
        assert!(!plan.entries[0].publish_needed);
        assert_eq!(plan.publish_count(), 0);
    }

    #[test]
    fn offline_skips_the_registry_and_still_publishes() {
        // Registry already reports 0.1.1, but --offline ignores the registry, so
        // idempotency is not anchored on it and a publish is still proposed.
        let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);
        let overrides = BumpOverrides::new().with_offline(true);
        let plan = plan_core(CommonEcosystemConfig::default(), target, &overrides);
        assert!(!plan.entries[0].up_to_date);
        assert!(plan.entries[0].publish_needed);
    }

    #[test]
    fn dependent_upgrade_raises_floor_without_own_bump() {
        let core = module("core", "crates/core");
        let app = module("app", "crates/app");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core.clone(), app.clone()];
        response.edges = vec![Edge::new(app.id.clone(), core.id, DepKind::Normal)];

        let mut common = CommonEcosystemConfig::default();
        common.release.dependent_version = Some(DependentVersion::Upgrade);
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        let overrides = BumpOverrides::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &overrides,
            &mut reporter,
        )
        .unwrap();

        let app_entry = plan
            .entries
            .iter()
            .find(|entry| entry.module == app.key())
            .expect("app entry");
        assert_eq!(app_entry.planned_version, None);
        assert!(!app_entry.publish_needed);
        assert_eq!(
            app_entry.mutation.dep_floor_updates.get(&mref("core")),
            Some(&Version::new(0, 1, 1))
        );
    }

    #[test]
    fn reconcile_policy_defaults_to_semver_cascade_when_no_modules() {
        let empty = BTreeMap::new();
        assert_eq!(reconcile_policy(&empty).unwrap(), BumpPolicy::SemverCascade);
    }

    #[test]
    fn resolved_settings_carry_the_single_policy() {
        let settings =
            ResolvedReleaseSettings::resolve(&toven_ports::ReleaseConfig::default(), None).unwrap();
        assert_eq!(settings.policy, BumpPolicy::SemverCascade);
    }
}
