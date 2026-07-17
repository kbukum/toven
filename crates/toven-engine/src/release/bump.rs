//! Release bump planning: resolve each module's independent bump from config and
//! the per-run argv overrides, cascade dependency floors, and pre-skip versions
//! already satisfied by the registry (or, offline, the release tag).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use toven_model::{DepKind, Edge, Graph, Module, ModuleKey, ModuleRef};
use toven_ports::{BumpLevel, DependentVersion, ReleaseMutation, ReleaseTarget};

use super::strategy::{self, EffectiveLevel};
use super::{
    BumpOverrides, BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseBaseline,
    ReleaseEntry, ResolvedReleaseSettings,
};

/// Inputs required to build release entries.
pub(super) struct BumpInputs<'a> {
    pub(super) graph: &'a Graph,
    pub(super) modules: &'a [Module],
    pub(super) edges: &'a [Edge],
    pub(super) changed: &'a BTreeSet<ModuleKey>,
    pub(super) baselines: &'a BTreeMap<ModuleKey, ReleaseBaseline>,
    pub(super) changelogs: &'a BTreeMap<ModuleKey, ChangelogEntry>,
    pub(super) settings: &'a BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    pub(super) targets: &'a super::ReleaseTargets,
    pub(super) policy: BumpPolicy,
    pub(super) overrides: &'a BumpOverrides,
}

/// The resolved own-version bump for one module, before idempotency pre-skip.
struct BumpDecision {
    planned: Option<Version>,
    level: BumpLevel,
    reason: BumpReason,
    winning_input: BumpSource,
    prerelease_channel: Option<String>,
}

/// One module's resolved bump, its dependency-floor updates, and its cascade
/// origin, prepared in dependency-first order before entry assembly.
struct PreparedBump {
    reference: ModuleKey,
    current: Version,
    origin: Option<ModuleKey>,
    decision: BumpDecision,
    dep_floor_updates: BTreeMap<ModuleRef, Version>,
}

/// Build release entries from changed modules and release targets.
///
/// Bumps are decided **dependency-first** so a dependent only cascades when a
/// direct dependency actually receives an own-version bump — a dependent whose
/// dependencies stayed put (e.g. an `upgrade`-mode intermediate that raised a
/// floor without republishing) is never given a bump that carries no change.
pub(super) fn plan_entries(input: &BumpInputs<'_>) -> AppResult<Vec<ReleaseEntry>> {
    let active = input.graph.closure(input.changed, release_closure_edge)?;
    let module_by_ref = input
        .modules
        .iter()
        .map(|module| (module.key(), module))
        .collect::<BTreeMap<_, _>>();

    input
        .overrides
        .validate_known(&active.iter().map(|key| key.module.clone()).collect())?;

    let ranks = publish_ranks(input.graph, &active)?;
    let mut ordered = active.iter().cloned().collect::<Vec<_>>();
    ordered.sort_by_key(|module| (*ranks.get(module).unwrap_or(&usize::MAX), module.clone()));

    let mut planned_versions: BTreeMap<ModuleKey, Version> = BTreeMap::new();
    let mut prepared = Vec::with_capacity(ordered.len());
    for reference in &ordered {
        let module = lookup(&module_by_ref, reference)?;
        let target = target_for(input.targets, module)?;
        let current = target.declared_version(module)?;
        // Every dependency has a lower topo rank, so its bump (if any) is already
        // recorded: a non-empty floor set means a direct dependency really bumped.
        let dep_floor_updates = dep_floor_updates(reference, input.edges, &planned_versions);
        let origin = cascade_origin(reference, input.graph, input.changed)?;
        let decision = resolve_bump(input, reference, &current, !dep_floor_updates.is_empty())?;
        if let Some(version) = &decision.planned {
            planned_versions.insert(reference.clone(), version.clone());
        }
        prepared.push(PreparedBump {
            reference: reference.clone(),
            current,
            origin,
            decision,
            dep_floor_updates,
        });
    }

    let mut entries = Vec::with_capacity(prepared.len());
    for PreparedBump {
        reference,
        current,
        origin,
        decision,
        dep_floor_updates,
    } in prepared
    {
        let module = lookup(&module_by_ref, &reference)?;
        let target = target_for(input.targets, module)?;
        let (up_to_date, publish_needed) =
            idempotency(input, module, target, &reference, decision.planned.as_ref());
        let cascade_origin = origin.filter(|_| decision.reason == BumpReason::DependencyCascade);
        let mutation = ReleaseMutation {
            new_version: decision.planned.clone(),
            dep_floor_updates,
        };
        entries.push(ReleaseEntry {
            module: reference.clone(),
            current_version: current,
            planned_version: decision.planned,
            level: decision.level,
            reason: decision.reason,
            winning_input: decision.winning_input,
            cascade_origin,
            prerelease_channel: decision.prerelease_channel,
            up_to_date,
            mutation,
            publish_needed,
            topo_rank: *ranks.get(&reference).unwrap_or(&usize::MAX),
            baseline: input.baselines.get(&reference).cloned(),
            changelog: input
                .changelogs
                .get(&reference)
                .cloned()
                .unwrap_or_else(|| {
                    ChangelogEntry::new(reference.clone(), "dependency cascade", Vec::new())
                }),
        });
    }
    entries.sort_by(|left, right| {
        left.topo_rank
            .cmp(&right.topo_rank)
            .then_with(|| left.module.cmp(&right.module))
    });
    Ok(entries)
}

/// Resolve one module's own-version bump under the documented precedence
/// (argv `--set-version` > argv level > config level > adapter default), then a
/// dependency cascade for a dependent that did not itself change.
fn resolve_bump(
    input: &BumpInputs<'_>,
    reference: &ModuleKey,
    current: &Version,
    is_cascade: bool,
) -> AppResult<BumpDecision> {
    let settings = input.settings.get(reference);
    let module_ref = &reference.module;

    if let Some(version) = input.overrides.set_version(module_ref) {
        return Ok(BumpDecision {
            level: classify(current, version),
            planned: Some(version.clone()),
            reason: BumpReason::Explicit,
            winning_input: BumpSource::SetVersion,
            prerelease_channel: None,
        });
    }

    let is_seed = input.changed.contains(reference);
    let breaking = input
        .changelogs
        .get(reference)
        .is_some_and(|entry| entry.breaking);
    let (level, winning_input, reason) =
        select_level(input, module_ref, settings, is_seed, is_cascade, breaking);

    let Some(level) = level else {
        // Dependency-floor upgrade: raise the floor but leave the own version.
        return Ok(BumpDecision {
            planned: None,
            level: BumpLevel::Patch,
            reason,
            winning_input,
            prerelease_channel: None,
        });
    };

    // Only a module cutting an own version consults the prerelease channel, so a
    // floor-only dependent never fails on a channel it would not use.
    let channel = resolve_channel(input, settings)?;
    let planned = strategy::next_version(input.policy, current, level, channel.as_deref())?;
    Ok(BumpDecision {
        planned: Some(planned),
        level: effective_to_level(level),
        reason,
        winning_input,
        prerelease_channel: channel,
    })
}

/// Select the effective bump level and its winning input/reason. `None` means a
/// dependency-floor upgrade with no own-version bump.
fn select_level(
    input: &BumpInputs<'_>,
    module_ref: &ModuleRef,
    settings: Option<&ResolvedReleaseSettings>,
    is_seed: bool,
    is_cascade: bool,
    breaking: bool,
) -> (Option<EffectiveLevel>, BumpSource, BumpReason) {
    if let Some(level) = input.overrides.module_level(module_ref) {
        let reason = if is_seed {
            BumpReason::Changed
        } else {
            BumpReason::DependencyCascade
        };
        return (Some(level_to_effective(level)), BumpSource::Argv, reason);
    }
    if is_seed {
        return match settings.map_or(BumpLevel::Auto, |resolved| resolved.level) {
            BumpLevel::Patch => (
                Some(EffectiveLevel::Patch),
                BumpSource::Config,
                BumpReason::Changed,
            ),
            BumpLevel::Minor => (
                Some(EffectiveLevel::Minor),
                BumpSource::Config,
                BumpReason::Changed,
            ),
            BumpLevel::Major => (
                Some(EffectiveLevel::Major),
                BumpSource::Config,
                BumpReason::Changed,
            ),
            BumpLevel::Auto if breaking => (
                Some(EffectiveLevel::Minor),
                BumpSource::Changelog,
                BumpReason::Changed,
            ),
            _ => (
                Some(EffectiveLevel::Patch),
                BumpSource::Default,
                BumpReason::Changed,
            ),
        };
    }
    if is_cascade {
        return match settings.map_or(DependentVersion::Bump, |resolved| {
            resolved.dependent_version
        }) {
            DependentVersion::Upgrade => (None, BumpSource::Cascade, BumpReason::DependencyCascade),
            _ => (
                Some(EffectiveLevel::Patch),
                BumpSource::Cascade,
                BumpReason::DependencyCascade,
            ),
        };
    }
    // Not changed and not a cascade dependent: a floor-only participant.
    (None, BumpSource::Cascade, BumpReason::DependencyCascade)
}

/// Resolve and validate the per-run prerelease channel against the module's
/// configured channels.
fn resolve_channel(
    input: &BumpInputs<'_>,
    settings: Option<&ResolvedReleaseSettings>,
) -> AppResult<Option<String>> {
    let Some(channel) = input.overrides.prerelease() else {
        return Ok(None);
    };
    let recognized = settings.is_some_and(|resolved| resolved.prerelease.recognizes(channel));
    if !recognized {
        return Err(AppError::invalid_input(
            "release.pre",
            format!("prerelease channel '{channel}' is not one of the configured channels"),
        ));
    }
    Ok(Some(channel.to_string()))
}

/// Classify the semver distance between `current` and an explicit `target`.
const fn classify(current: &Version, target: &Version) -> BumpLevel {
    if target.major != current.major {
        BumpLevel::Major
    } else if target.minor != current.minor {
        BumpLevel::Minor
    } else {
        BumpLevel::Patch
    }
}

const fn level_to_effective(level: BumpLevel) -> EffectiveLevel {
    match level {
        BumpLevel::Minor => EffectiveLevel::Minor,
        BumpLevel::Major => EffectiveLevel::Major,
        _ => EffectiveLevel::Patch,
    }
}

const fn effective_to_level(level: EffectiveLevel) -> BumpLevel {
    match level {
        EffectiveLevel::Patch => BumpLevel::Patch,
        EffectiveLevel::Minor => BumpLevel::Minor,
        EffectiveLevel::Major => BumpLevel::Major,
    }
}

/// Decide `(up_to_date, publish_needed)` for a planned version, anchoring on the
/// registry's published set (or, offline, the baseline release tag).
fn idempotency(
    input: &BumpInputs<'_>,
    module: &Module,
    target: &dyn ReleaseTarget,
    reference: &ModuleKey,
    planned: Option<&Version>,
) -> (bool, bool) {
    let Some(planned) = planned else {
        // A floor-only upgrade never publishes an own version.
        return (false, false);
    };
    let offline = input.overrides.offline()
        || input
            .settings
            .get(reference)
            .is_some_and(|resolved| resolved.offline);
    if offline {
        let up_to_date = input
            .baselines
            .get(reference)
            .and_then(|baseline| baseline.version.as_ref())
            .is_some_and(|tagged| planned <= tagged);
        return (up_to_date, !up_to_date);
    }
    // `published_versions` is best-effort: a transient registry/search failure
    // must not abort planning. Treat a lookup error as "publish needed" — the
    // APPLY publish loop's `AlreadyPublished` classification is the authoritative
    // idempotency backstop.
    let Ok(published) = target.published_versions(module) else {
        return (false, true);
    };
    let up_to_date = published.iter().max().is_some_and(|max| planned <= max);
    (up_to_date, !up_to_date)
}

/// The changed module that triggered a dependent's cascade, if the module is a
/// dependent (not itself changed) that transitively depends on a changed module.
///
/// The cascade closure is transitive over the dependency graph, so a dependent
/// several hops removed from the change (`A` changed → `B` → `C`) still resolves
/// its origin to the changed root, matching the transitive reverse-dependents
/// closure that selects the release scope.
fn cascade_origin(
    module: &ModuleKey,
    graph: &Graph,
    changed: &BTreeSet<ModuleKey>,
) -> AppResult<Option<ModuleKey>> {
    if changed.contains(module) {
        return Ok(None);
    }
    let seed: BTreeSet<ModuleKey> = std::iter::once(module.clone()).collect();
    let dependencies = graph.dependencies_closure(&seed, release_closure_edge)?;
    Ok(dependencies
        .into_iter()
        .filter(|dependency| changed.contains(dependency))
        .min())
}

const fn release_closure_edge(kind: DepKind) -> bool {
    !matches!(kind, DepKind::Overlay)
}

fn lookup<'a>(
    module_by_ref: &BTreeMap<ModuleKey, &'a Module>,
    reference: &ModuleKey,
) -> AppResult<&'a Module> {
    module_by_ref.get(reference).copied().ok_or_else(|| {
        AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
    })
}

fn target_for<'a>(
    targets: &'a super::ReleaseTargets,
    module: &Module,
) -> AppResult<&'a dyn ReleaseTarget> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.target",
                format!("module '{}' has no release target", module.key()),
            )
        })
}

fn dep_floor_updates(
    module: &ModuleKey,
    edges: &[Edge],
    planned_versions: &BTreeMap<ModuleKey, Version>,
) -> BTreeMap<ModuleRef, Version> {
    edges
        .iter()
        .filter(|edge| {
            &edge.from == module
                && edge.from.module.ecosystem == edge.to.module.ecosystem
                && edge.from.member == edge.to.member
                && !matches!(edge.kind, DepKind::Overlay)
        })
        .filter_map(|edge| {
            planned_versions
                .get(&edge.to)
                .map(|version| (edge.to.module.clone(), version.clone()))
        })
        .collect()
}

fn publish_ranks(
    graph: &Graph,
    active: &BTreeSet<ModuleKey>,
) -> AppResult<BTreeMap<ModuleKey, usize>> {
    let waves = graph.waves(|edge| active.contains(&edge.from) && active.contains(&edge.to))?;
    let mut ranks = BTreeMap::new();
    let mut rank = 0;
    for wave in waves {
        for module in wave {
            if active.contains(&module) {
                ranks.insert(module, rank);
                rank += 1;
            }
        }
    }
    Ok(ranks)
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{DepKind, EcosystemId, Edge, Graph, Module, RepoPath};
    use toven_ports::{BumpLevel, DependentVersion, ReleaseConfig, ReleaseTarget};
    use toven_testkit::FakeReleaseTarget;

    use super::{
        BTreeMap, BTreeSet, BumpInputs, BumpOverrides, BumpPolicy, BumpSource, ChangelogEntry,
        ModuleRef, ResolvedReleaseSettings, plan_entries,
    };
    use crate::release::ReleaseTargets;

    fn core_module() -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
            RepoPath::new("crates/core").unwrap(),
        )
    }

    fn rust_module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            RepoPath::new(format!("crates/{name}")).unwrap(),
        )
    }

    fn settings_for(config: &ReleaseConfig) -> ResolvedReleaseSettings {
        ResolvedReleaseSettings::resolve(config, None).unwrap()
    }

    fn rust_targets() -> ReleaseTargets {
        let mut targets = ReleaseTargets::new();
        targets.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(FakeReleaseTarget::new()) as Box<dyn ReleaseTarget>,
        );
        targets
    }

    #[test]
    fn a_breaking_changelog_classification_forces_a_minor_bump() {
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();

        let mut targets = ReleaseTargets::new();
        targets.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(FakeReleaseTarget::new()) as Box<dyn ReleaseTarget>,
        );

        let mut settings = BTreeMap::new();
        settings.insert(
            key.clone(),
            ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap(),
        );

        // Level resolves to `auto`; the breaking classification, not raw argv,
        // lifts it to a minor bump attributed to the changelog signal.
        let mut changelogs = BTreeMap::new();
        changelogs.insert(
            key.clone(),
            ChangelogEntry::new(key.clone(), "breaking change", Vec::new()).with_breaking(true),
        );

        let changed: BTreeSet<_> = std::iter::once(key).collect();
        let baselines = BTreeMap::new();
        let modules = vec![core];
        let edges = Vec::new();
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            edges: &edges,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, BumpLevel::Minor);
        assert_eq!(entries[0].winning_input, BumpSource::Changelog);
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 2, 0)));
    }

    #[test]
    fn an_upgrade_only_dependency_does_not_cascade_a_bump_to_its_dependents() {
        // app -> lib -> base; base changes, lib only raises floors (upgrade), so
        // app's direct dependency never republishes and app must stay untouched.
        let base = rust_module("base");
        let lib = rust_module("lib");
        let app = rust_module("app");
        let (base_key, lib_key, app_key) = (base.key(), lib.key(), app.key());
        let edges = vec![
            Edge::new(app_key.clone(), lib_key.clone(), DepKind::Normal),
            Edge::new(lib_key.clone(), base_key.clone(), DepKind::Normal),
        ];
        let modules = vec![base, lib, app];
        let graph = Graph::build(modules.clone(), edges.clone()).unwrap();

        let upgrade = ReleaseConfig {
            dependent_version: Some(DependentVersion::Upgrade),
            ..ReleaseConfig::default()
        };
        let mut settings = BTreeMap::new();
        settings.insert(base_key.clone(), settings_for(&ReleaseConfig::default()));
        settings.insert(lib_key.clone(), settings_for(&upgrade));
        settings.insert(app_key.clone(), settings_for(&ReleaseConfig::default()));

        let changed: BTreeSet<_> = std::iter::once(base_key.clone()).collect();
        let targets = rust_targets();
        let baselines = BTreeMap::new();
        let changelogs = BTreeMap::new();
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            edges: &edges,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
        })
        .unwrap();

        let by_module = |key: &_| entries.iter().find(|e| &e.module == key).unwrap();
        assert_eq!(
            by_module(&base_key).planned_version,
            Some(Version::new(0, 1, 1))
        );
        let lib_entry = by_module(&lib_key);
        assert_eq!(lib_entry.planned_version, None);
        assert!(!lib_entry.mutation.dep_floor_updates.is_empty());
        let app_entry = by_module(&app_key);
        assert_eq!(app_entry.planned_version, None);
        assert!(app_entry.mutation.dep_floor_updates.is_empty());
        assert!(!app_entry.publish_needed);
    }

    #[test]
    fn a_bumping_dependency_chain_cascades_through_every_dependent() {
        // app -> lib -> base with the default bump policy: base changes and each
        // dependent republishes, so the cascade reaches app transitively.
        let base = rust_module("base");
        let lib = rust_module("lib");
        let app = rust_module("app");
        let (base_key, lib_key, app_key) = (base.key(), lib.key(), app.key());
        let edges = vec![
            Edge::new(app_key.clone(), lib_key.clone(), DepKind::Normal),
            Edge::new(lib_key.clone(), base_key.clone(), DepKind::Normal),
        ];
        let modules = vec![base, lib, app];
        let graph = Graph::build(modules.clone(), edges.clone()).unwrap();

        let mut settings = BTreeMap::new();
        for key in [&base_key, &lib_key, &app_key] {
            settings.insert(key.clone(), settings_for(&ReleaseConfig::default()));
        }

        let changed: BTreeSet<_> = std::iter::once(base_key.clone()).collect();
        let targets = rust_targets();
        let baselines = BTreeMap::new();
        let changelogs = BTreeMap::new();
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            edges: &edges,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
        })
        .unwrap();

        let by_module = |key: &_| entries.iter().find(|e| &e.module == key).unwrap();
        assert_eq!(
            by_module(&base_key).planned_version,
            Some(Version::new(0, 1, 1))
        );
        assert_eq!(
            by_module(&lib_key).planned_version,
            Some(Version::new(0, 1, 1))
        );
        let app_entry = by_module(&app_key);
        assert_eq!(app_entry.planned_version, Some(Version::new(0, 1, 1)));
        assert!(!app_entry.mutation.dep_floor_updates.is_empty());
        assert!(app_entry.publish_needed);
    }
}
