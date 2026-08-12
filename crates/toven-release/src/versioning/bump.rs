//! Release entry assembly: gather each module's pre-decided inputs, run the pure
//! [`plan_bumps`](toven_version::plan_bumps) decision, and compose the resolved
//! [`ReleaseEntry`] set from its [`BumpPlan`] plus resolved settings and tag
//! formatting.
//!
//! The version *decision* (bump precedence, cascade, idempotency, baseline
//! anchoring) lives in the pure `toven-version` crate. This module owns the
//! impure ends around it: GATHER (reading each module's declared and published
//! versions from its ecosystem adapter) and MUTATE-side assembly (resolving the
//! planned tag, dependency-floor import edits, and the ~19 resolved-settings
//! fields a `ReleaseEntry` carries).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{DepKind, Graph, MemberId, Module, ModuleKey};
use toven_ports::{PublicationPolicy, ReleaseAdapter, ReleaseMutation, TagSigner};
use toven_version::{
    BumpConfig, BumpEntry, BumpPlan, ModuleVersionConfig, VersionInputs, plan_bumps,
};

use crate::model::tag;
use crate::{
    BumpOverrides, BumpPolicy, ChangelogEntry, PushPolicy, ReleaseBaseline, ReleaseEntry,
    ResolvedReleaseSettings,
};

/// The GATHER-side view of the release-eligible modules, before the pure
/// decision. Re-exported so change detection can share the [`CutIntent`].
pub(crate) use toven_version::CutIntent;

/// Inputs required to build release entries.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct BumpInputs<'a> {
    pub(crate) graph: &'a Graph,
    pub(crate) modules: &'a [Module],
    pub(crate) changed: &'a BTreeSet<ModuleKey>,
    pub(crate) baselines: &'a BTreeMap<ModuleKey, ReleaseBaseline>,
    pub(crate) changelogs: &'a BTreeMap<ModuleKey, ChangelogEntry>,
    pub(crate) settings: &'a BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    pub(crate) targets: &'a crate::ReleaseTargets,
    /// Checked-out branch per federation member (absent on detached HEAD),
    /// consulted only to resolve a configured branch→prerelease-channel mapping.
    pub(crate) branches: &'a BTreeMap<Option<MemberId>, String>,
    pub(crate) policy: BumpPolicy,
    pub(crate) overrides: &'a BumpOverrides,
    /// Whether this cut is a read-only projection, a `bump`, or a
    /// verify-and-publish run — see [`CutIntent`].
    pub(crate) intent: CutIntent,
}

/// Build release entries from changed modules and release targets.
///
/// GATHER reads each release-eligible module's declared version and (unless
/// offline) its registry-published versions from its ecosystem adapter into a
/// pure [`VersionInputs`]; the pure [`plan_bumps`] decides every module's bump,
/// cascade, and idempotency; and assembly composes the resolved
/// [`ReleaseEntry`] set from that [`BumpPlan`] plus resolved settings and tag
/// formatting.
///
/// # Errors
/// Propagates a missing release target for a module in the release closure, a
/// declared-version read failure, an invalid override combination, a graph
/// failure, or a tag-scheme failure surfaced while formatting a planned tag.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn plan_entries(input: &BumpInputs<'_>) -> AppResult<Vec<ReleaseEntry>> {
    let module_by_ref = input
        .modules
        .iter()
        .map(|module| (module.key(), module))
        .collect::<BTreeMap<_, _>>();

    let inputs = gather_inputs(input, &module_by_ref)?;
    let plan = plan_bumps(
        &inputs,
        &BumpConfig {
            graph: input.graph,
            branches: input.branches,
            policy: input.policy,
            overrides: input.overrides,
            intent: input.intent,
        },
    )?;
    assemble_entries(input, &module_by_ref, &plan)
}

/// Gather the pure decision inputs for every module in the release closure: its
/// adapter-declared version, its registry-published versions (empty offline),
/// its pre-resolved baseline, its change/breaking flags, and the
/// decision-relevant slice of its resolved config.
///
/// The closure is the non-overlay dependency expansion of `input.changed` — the
/// exact set the pure [`plan_bumps`] walks — so GATHER never reads a declared or
/// published version for a module that cannot enter the plan (an unrelated
/// module's read failure must not abort this release, and large workspaces skip
/// registry lookups for out-of-closure modules). A closure module with no
/// release target surfaces the typed missing-target error rather than being
/// silently dropped before the decision derives its changed seeds.
fn gather_inputs(
    input: &BumpInputs<'_>,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
) -> AppResult<Vec<VersionInputs>> {
    let closure = input
        .graph
        .closure(input.changed, |kind| !matches!(kind, DepKind::Overlay))?;
    let mut inputs = Vec::with_capacity(closure.len());
    for key in &closure {
        let module = lookup(module_by_ref, key)?;
        let target = target_for(input.targets, module).ok_or_else(|| missing_target(module))?;
        let settings = input.settings.get(key);
        let current_version = target.declared_version(module)?;
        let offline =
            input.overrides.offline() || settings.is_some_and(|resolved| resolved.offline);
        // Honor the offline contract: never reach the registry for a module the
        // run marked offline — its idempotency anchors on the release tag alone.
        let published_versions = if offline {
            Vec::new()
        } else {
            target.published_versions(module).unwrap_or_default()
        };
        let baseline = input
            .baselines
            .get(key)
            .cloned()
            .unwrap_or_else(|| ReleaseBaseline::initial(key.clone()));
        inputs.push(VersionInputs {
            module: key.clone(),
            current_version,
            published_versions,
            baseline,
            changed: input.changed.contains(key),
            breaking: input
                .changelogs
                .get(key)
                .is_some_and(|entry| entry.breaking),
            config: module_config(settings),
        });
    }
    Ok(inputs)
}

/// Project the decision-relevant slice of a module's resolved release config,
/// defaulting an unresolved module to the same defaults the resolver applies.
fn module_config(settings: Option<&ResolvedReleaseSettings>) -> ModuleVersionConfig {
    settings.map_or_else(
        || ModuleVersionConfig {
            level: toven_ports::BumpLevel::Auto,
            dependent_version: toven_ports::DependentVersion::Bump,
            prerelease: toven_ports::PrereleaseConfig::default(),
            publication: PublicationPolicy::TagOnly,
            offline: false,
            entrypoint: toven_model::Entrypoint::default(),
        },
        |resolved| ModuleVersionConfig {
            level: resolved.level,
            dependent_version: resolved.dependent_version,
            prerelease: resolved.prerelease.clone(),
            publication: resolved.publication.clone(),
            offline: resolved.offline,
            entrypoint: resolved.entrypoint,
        },
    )
}

/// Compose the resolved [`ReleaseEntry`] set from the pure [`BumpPlan`] plus
/// resolved settings and tag formatting.
///
/// Every entry the plan produced already carries its own-version bump and
/// dependency floors; assembly resolves the planned tag (surfacing a tag-scheme
/// failure at plan time), maps floor updates to their import package names, and
/// copies the resolved-settings fields a mutating run needs.
///
/// # Errors
/// Propagates a missing release target for a planned module or a tag-scheme
/// failure surfaced while formatting a planned tag.
fn assemble_entries(
    input: &BumpInputs<'_>,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    plan: &BumpPlan,
) -> AppResult<Vec<ReleaseEntry>> {
    let mut entries = Vec::with_capacity(plan.entries.len());
    for bump in &plan.entries {
        entries.push(assemble_entry(input, module_by_ref, bump)?);
    }
    Ok(entries)
}

/// Compose a single [`ReleaseEntry`] from one planned [`BumpEntry`], resolving
/// its planned tag and copying the resolved-settings fields a mutating run
/// needs.
///
/// # Errors
/// Propagates a missing release target for the planned module or a tag-scheme
/// failure surfaced while formatting its planned tag.
fn assemble_entry(
    input: &BumpInputs<'_>,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    bump: &BumpEntry,
) -> AppResult<ReleaseEntry> {
    let reference = &bump.module;
    let module = lookup(module_by_ref, reference)?;
    let target = target_for(input.targets, module).ok_or_else(|| missing_target(module))?;
    let resolved = input.settings.get(reference);
    let publication = resolved.map_or(PublicationPolicy::TagOnly, |r| r.publication.clone());
    let tag_format = resolved.and_then(|r| r.tag_format.clone());
    let tag_mode = resolved.and_then(|r| r.tag_mode);
    let baseline_source = resolved.and_then(|r| r.baseline);
    // Resolve the planned tag now so the plan explains the exact tag a
    // mutating run would create — and so a tag-scheme failure surfaces at
    // plan time rather than mid-mutation.
    let planned_tag = bump
        .planned_version
        .as_ref()
        .map(|version| {
            target
                .tag_scheme(module, tag_format.as_deref())
                .map(|scheme| tag::format(&scheme, version))
        })
        .transpose()?;
    let dep_floor_import_updates = bump
        .dep_floor_updates
        .iter()
        .filter_map(|(dependency, version)| {
            input
                .modules
                .iter()
                .find(|module| module.id == *dependency)
                .and_then(|module| module.package.clone())
                .map(|package| (package, version.clone()))
        })
        .collect();
    let mutation = ReleaseMutation {
        new_version: bump.planned_version.clone(),
        dep_floor_updates: bump.dep_floor_updates.clone(),
        dep_floor_import_updates,
    };
    Ok(ReleaseEntry {
        module: reference.clone(),
        current_version: bump.current_version.clone(),
        planned_version: bump.planned_version.clone(),
        planned_tag,
        level: bump.level,
        reason: bump.reason,
        winning_input: bump.winning_input,
        cascade_origin: bump.cascade_origin.clone(),
        prerelease_channel: bump.prerelease_channel.clone(),
        up_to_date: bump.up_to_date,
        mutation,
        publication,
        publish_needed: bump.publish_needed,
        tag_format,
        tag_mode,
        baseline_source,
        tag_message: resolved.and_then(|r| r.tag_message.clone()),
        signer: resolved.filter(|r| r.sign_tags).map(|r| TagSigner {
            format: r.sign_format,
            key: r.signing_key.clone(),
        }),
        commit_message: resolved.and_then(|r| r.commit_message.clone()),
        token_env: resolved.and_then(|r| r.token_env.clone()),
        visibility: resolved.map_or_else(Default::default, |r| r.visibility),
        push: resolved.map_or(PushPolicy::BranchAndTags, |r| r.push),
        remote: resolved.map_or_else(|| "origin".to_string(), |r| r.remote.clone()),
        branches: resolved.map_or_else(Vec::new, |r| r.branches.clone()),
        topo_rank: bump.topo_rank,
        baseline: input.baselines.get(reference).cloned(),
        changelog: input.changelogs.get(reference).cloned().unwrap_or_else(|| {
            ChangelogEntry::new(reference.clone(), "dependency cascade", Vec::new())
        }),
        changelog_path: resolved
            .and_then(|r| r.changelog.path.clone())
            .unwrap_or_else(|| "CHANGELOG.md".to_string()),
        changelog_roll: resolved.is_some_and(|r| r.changelog.roll),
        entrypoint: resolved.map_or_else(Default::default, |r| r.entrypoint),
        umbrella: resolved.is_some_and(|r| r.umbrella),
        version_references: resolved.map_or_else(Vec::new, |r| r.version_references.clone()),
        on_resolved: resolved.map_or_else(Vec::new, |r| r.on_resolved.clone()),
    })
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
    targets: &'a crate::ReleaseTargets,
    module: &Module,
) -> Option<&'a dyn ReleaseAdapter> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
}

fn missing_target(module: &Module) -> AppError {
    AppError::invalid_input(
        "release.target",
        format!("module '{}' has no release target", module.key()),
    )
}

#[cfg(test)]
mod tests {
    use rskit_errors::AppResult;
    use rskit_version::semver::Version;
    use toven_model::{
        DepKind, EcosystemId, Edge, Graph, MemberId, Module, ModuleKey, ModuleRef, RepoPath,
    };
    use toven_ports::{BumpLevel, DependentVersion, Oid, ReleaseAdapter, ReleaseConfig};
    use toven_testkit::FakeReleaseTarget;

    use super::{BTreeMap, BTreeSet, BumpInputs, CutIntent, plan_entries};
    use crate::ReleaseTargets;
    use crate::{
        BumpOverrides, BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseBaseline,
        ReleaseEntry, ResolvedReleaseSettings,
    };

    /// The empty per-member branch map: tests that do not exercise
    /// branch→channel mapping resolve no branch-derived prerelease channel.
    fn no_branches() -> BTreeMap<Option<MemberId>, String> {
        BTreeMap::new()
    }

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
            Box::new(FakeReleaseTarget::new()) as Box<dyn ReleaseAdapter>,
        );
        targets
    }

    /// A released tag baseline anchoring `key` at `version` — the resolved
    /// baseline GATHER supplies for an already-released module.
    fn released(key: &ModuleKey, version: Version) -> ReleaseBaseline {
        ReleaseBaseline::tag(
            key.clone(),
            format!("v{version}"),
            version,
            Oid::new("cafe"),
        )
    }

    #[test]
    fn a_breaking_changelog_classification_forces_a_minor_bump() {
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();

        let mut targets = ReleaseTargets::new();
        targets.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(FakeReleaseTarget::new()) as Box<dyn ReleaseAdapter>,
        );

        let mut settings = BTreeMap::new();
        settings.insert(
            key.clone(),
            ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap(),
        );

        // Level resolves to `auto`; the breaking classification, not raw argv, lifts it
        // to a minor bump attributed to the changelog signal.
        let mut changelogs = BTreeMap::new();
        changelogs.insert(
            key.clone(),
            ChangelogEntry::new(key.clone(), "breaking change", Vec::new()).with_breaking(true),
        );

        let changed: BTreeSet<_> = std::iter::once(key.clone()).collect();
        let mut baselines = BTreeMap::new();
        baselines.insert(key.clone(), released(&key, Version::new(0, 1, 0)));
        let modules = vec![core];
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, BumpLevel::Minor);
        assert_eq!(entries[0].winning_input, BumpSource::Changelog);
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 2, 0)));
    }

    #[test]
    fn a_set_version_at_or_below_the_current_version_is_rejected() {
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();
        let targets = rust_targets();

        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&ReleaseConfig::default()));

        // The fake target declares 0.1.0, so pinning that same version is a no-op
        // rewrite and must be rejected before it can reach APPLY.
        let overrides = BumpOverrides::new()
            .with_set_version(core.id.clone(), Version::new(0, 1, 0))
            .unwrap();
        let changed: BTreeSet<_> = std::iter::once(key).collect();
        let baselines = BTreeMap::new();
        let changelogs = BTreeMap::new();
        let modules = vec![core];

        let result = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        });

        assert!(result.is_err());
    }

    #[test]
    fn a_maintainer_owned_module_plans_its_declared_version_from_the_manifest() {
        // A maintainer-owned module publishes the version its manifest already
        // declares (the fake target reports 0.1.0) against the tag a maintainer
        // cut: planning neither computes nor moves the version — it plans exactly
        // the declared version, attributed to the manifest, so APPLY can verify
        // the tag and publish idempotently.
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();
        let targets = rust_targets();

        let maintainer = ReleaseConfig {
            entrypoint: Some(toven_model::Entrypoint::Maintainer),
            ..ReleaseConfig::default()
        };
        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&maintainer));

        // The module carries the declared version even though change detection
        // seeded it (the flow forces a maintainer-owned module in regardless of
        // commits since the baseline).
        let changed: BTreeSet<_> = std::iter::once(key).collect();
        let baselines = BTreeMap::new();
        let changelogs = BTreeMap::new();
        let modules = vec![core];
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 0)));
        assert_eq!(entries[0].reason, BumpReason::Manifest);
        assert_eq!(entries[0].winning_input, BumpSource::Manifest);
        assert!(entries[0].entrypoint.is_maintainer_owned());
    }

    #[test]
    fn a_maintainer_owned_module_fails_closed_when_declared_version_is_behind_the_baseline() {
        // The manifest declares 0.1.0 but the module already released 0.2.0
        // (baseline). A maintainer-owned mutate must not republish a version
        // behind the latest release, so planning fails closed rather than
        // planning the regressed version.
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();
        let targets = rust_targets();

        let maintainer = ReleaseConfig {
            entrypoint: Some(toven_model::Entrypoint::Maintainer),
            ..ReleaseConfig::default()
        };
        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&maintainer));

        let changed: BTreeSet<_> = std::iter::once(key.clone()).collect();
        let mut baselines = BTreeMap::new();
        baselines.insert(
            key.clone(),
            ReleaseBaseline::tag(
                key,
                "rust/core@0.2.0",
                Version::new(0, 2, 0),
                Oid::new("cafe"),
            ),
        );
        let changelogs = BTreeMap::new();
        let modules = vec![core];
        let overrides = BumpOverrides::new();

        let error = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        })
        .expect_err("a maintainer-owned version behind the baseline fails closed");

        let message = error.to_string();
        assert!(
            message.contains("behind the released baseline"),
            "{message}"
        );
        assert!(message.contains("0.2.0"), "{message}");
    }

    #[test]
    fn a_maintainer_owned_module_plans_the_baseline_version_for_an_idempotent_rerun() {
        // The manifest declares exactly the released baseline (0.1.0). A
        // maintainer-owned re-run is the steady state: it plans that version so
        // APPLY verifies the tag and registry idempotency decides publish — the
        // baseline floor allows `current == baseline`, only rejecting a regress.
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();
        let targets = rust_targets();

        let maintainer = ReleaseConfig {
            entrypoint: Some(toven_model::Entrypoint::Maintainer),
            ..ReleaseConfig::default()
        };
        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&maintainer));

        let changed: BTreeSet<_> = std::iter::once(key.clone()).collect();
        let mut baselines = BTreeMap::new();
        baselines.insert(
            key.clone(),
            ReleaseBaseline::tag(
                key,
                "rust/core@0.1.0",
                Version::new(0, 1, 0),
                Oid::new("cafe"),
            ),
        );
        let changelogs = BTreeMap::new();
        let modules = vec![core];
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        })
        .expect("a maintainer-owned re-run at the baseline version is allowed");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 0)));
        assert_eq!(entries[0].reason, BumpReason::Manifest);
    }

    #[test]
    fn a_maintainer_owned_module_computes_a_bump_on_the_bump_path() {
        // The gap-2 regression: under `entrypoint = "maintainer"`, `release bump`
        // (CutIntent::Bump) must still COMPUTE a change-gated increment, not echo
        // the declared version. The crate declares exactly its released baseline
        // (0.1.0) and changed since, so bump advances it to 0.1.1 — the version a
        // maintainer then reviews and merges. Only `Verify` (tag/publish) echoes
        // the already-merged declared version.
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();
        let targets = rust_targets();

        let maintainer = ReleaseConfig {
            entrypoint: Some(toven_model::Entrypoint::Maintainer),
            ..ReleaseConfig::default()
        };
        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&maintainer));

        let changed: BTreeSet<_> = std::iter::once(key.clone()).collect();
        let mut baselines = BTreeMap::new();
        baselines.insert(
            key.clone(),
            ReleaseBaseline::tag(
                key,
                "rust/core@0.1.0",
                Version::new(0, 1, 0),
                Oid::new("cafe"),
            ),
        );
        let changelogs = BTreeMap::new();
        let modules = vec![core];
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Bump,
        })
        .expect("a maintainer-owned bump computes a change-gated increment");

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].planned_version,
            Some(Version::new(0, 1, 1)),
            "bump must compute a real increment under maintainer entrypoint, not echo 0.1.0"
        );
        assert_eq!(entries[0].reason, BumpReason::Changed);
        assert!(entries[0].entrypoint.is_maintainer_owned());
    }

    #[test]
    fn an_upgrade_only_dependency_does_not_cascade_a_bump_to_its_dependents() {
        // app -> lib -> base; base changes, lib only raises floors (upgrade), so app's
        // direct dependency never republishes and app must stay untouched.
        let base = rust_module("base");
        let lib = rust_module("lib");
        let app = rust_module("app");
        let (base_key, lib_key, app_key) = (base.key(), lib.key(), app.key());
        let edges = vec![
            Edge::new(app_key.clone(), lib_key.clone(), DepKind::Normal),
            Edge::new(lib_key.clone(), base_key.clone(), DepKind::Normal),
        ];
        let modules = vec![base, lib, app];
        let graph = Graph::build(modules.clone(), edges).unwrap();

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
        let mut baselines = BTreeMap::new();
        for key in [&base_key, &lib_key, &app_key] {
            baselines.insert(key.clone(), released(key, Version::new(0, 1, 0)));
        }
        let changelogs = BTreeMap::new();
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        })
        .unwrap();

        let by_module = |key: &_| entries.iter().find(|e| &e.module == key);
        assert_eq!(
            by_module(&base_key).unwrap().planned_version,
            Some(Version::new(0, 1, 1))
        );
        let lib_entry = by_module(&lib_key).unwrap();
        assert_eq!(lib_entry.planned_version, None);
        assert!(!lib_entry.mutation.dep_floor_updates.is_empty());
        // app's only dependency raised a floor without republishing, so app has no
        // mutation and is dropped from the plan entirely.
        assert!(by_module(&app_key).is_none());
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
        let graph = Graph::build(modules.clone(), edges).unwrap();

        let mut settings = BTreeMap::new();
        let registry = ReleaseConfig {
            registry: Some("crates-io".into()),
            ..ReleaseConfig::default()
        };
        for key in [&base_key, &lib_key, &app_key] {
            settings.insert(key.clone(), settings_for(&registry));
        }

        let changed: BTreeSet<_> = std::iter::once(base_key.clone()).collect();
        let targets = rust_targets();
        let mut baselines = BTreeMap::new();
        for key in [&base_key, &lib_key, &app_key] {
            baselines.insert(key.clone(), released(key, Version::new(0, 1, 0)));
        }
        let changelogs = BTreeMap::new();
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
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

    /// Plan a single `rust/core` seed whose manifest declares `declared`, with an
    /// optional released baseline version, under `policy` + `overrides`. Defaults
    /// to a mutating cut so the `manifest` not-ahead guard fails closed.
    fn seed_plan(
        declared: &str,
        baseline: Option<&str>,
        policy: BumpPolicy,
        overrides: &BumpOverrides,
    ) -> AppResult<Vec<ReleaseEntry>> {
        seed_plan_with_intent(declared, baseline, policy, overrides, CutIntent::Verify)
    }

    /// Plan a single `rust/core` seed as [`seed_plan`], choosing the cut
    /// `intent` explicitly so previews can be distinguished from mutating runs.
    fn seed_plan_with_intent(
        declared: &str,
        baseline: Option<&str>,
        policy: BumpPolicy,
        overrides: &BumpOverrides,
        intent: CutIntent,
    ) -> AppResult<Vec<ReleaseEntry>> {
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();

        let mut targets = ReleaseTargets::new();
        targets.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(
                FakeReleaseTarget::new().with_declared_version(Version::parse(declared).unwrap()),
            ) as Box<dyn ReleaseAdapter>,
        );

        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&ReleaseConfig::default()));

        let mut baselines = BTreeMap::new();
        if let Some(version) = baseline {
            let parsed = Version::parse(version).unwrap();
            baselines.insert(
                key.clone(),
                ReleaseBaseline::tag(
                    key.clone(),
                    format!("rust/core@{parsed}"),
                    parsed,
                    Oid::new("cafe"),
                ),
            );
        } else {
            baselines.insert(key.clone(), ReleaseBaseline::initial(key.clone()));
        }

        let changed: BTreeSet<_> = std::iter::once(key).collect();
        let changelogs = BTreeMap::new();
        let modules = vec![core];

        plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy,
            overrides,
            intent,
        })
    }

    #[test]
    fn manifest_cuts_the_declared_prerelease_when_it_is_ahead_of_the_baseline() {
        let entries = seed_plan(
            "0.1.0-alpha.2",
            Some("0.1.0-alpha.1"),
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].planned_version,
            Some(Version::parse("0.1.0-alpha.2").unwrap())
        );
        assert_eq!(entries[0].reason, BumpReason::Manifest);
        assert_eq!(entries[0].winning_input, BumpSource::Manifest);
    }

    #[test]
    fn manifest_finalizes_a_declared_release_over_a_prerelease_baseline() {
        let entries = seed_plan(
            "0.1.0",
            Some("0.1.0-alpha.2"),
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 0)));
        assert_eq!(entries[0].winning_input, BumpSource::Manifest);
    }

    #[test]
    fn manifest_cuts_a_declared_plain_patch() {
        let entries = seed_plan(
            "0.1.1",
            Some("0.1.0"),
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 1)));
    }

    #[test]
    fn manifest_fails_closed_on_a_mutating_run_when_the_version_equals_the_baseline() {
        let error = seed_plan(
            "0.1.0-alpha.2",
            Some("0.1.0-alpha.2"),
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("not ahead"));
    }

    #[test]
    fn manifest_fails_closed_on_a_mutating_run_when_the_version_is_behind_the_baseline() {
        assert!(
            seed_plan(
                "0.1.0-alpha.2",
                Some("0.1.0"),
                BumpPolicy::Manifest,
                &BumpOverrides::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn manifest_preview_is_a_no_op_when_the_version_is_not_ahead_of_the_baseline() {
        // A read-only projection of a not-ahead manifest version reports nothing
        // to release rather than failing closed, so `release plan` stays safe to
        // run anywhere (equal baseline and a behind baseline both drop out).
        let equal = seed_plan_with_intent(
            "0.1.0-alpha.2",
            Some("0.1.0-alpha.2"),
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
            CutIntent::Preview,
        )
        .unwrap();
        assert!(equal.is_empty(), "equal-baseline preview must be a no-op");

        let behind = seed_plan_with_intent(
            "0.1.0-alpha.2",
            Some("0.1.0"),
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
            CutIntent::Preview,
        )
        .unwrap();
        assert!(behind.is_empty(), "behind-baseline preview must be a no-op");
    }

    #[test]
    fn set_version_still_overrides_the_manifest_policy() {
        let core = core_module();
        let overrides = BumpOverrides::new()
            .with_set_version(core.id, Version::new(0, 2, 0))
            .unwrap();

        let entries = seed_plan("0.1.0", Some("0.1.0"), BumpPolicy::Manifest, &overrides).unwrap();

        assert_eq!(entries[0].planned_version, Some(Version::new(0, 2, 0)));
        assert_eq!(entries[0].winning_input, BumpSource::SetVersion);
    }

    #[test]
    fn an_argv_level_override_takes_the_computed_path_under_manifest() {
        let core = core_module();
        let overrides = BumpOverrides::new()
            .with_module_level(core.id, BumpLevel::Minor)
            .unwrap();

        let entries = seed_plan("0.1.0", Some("0.1.0"), BumpPolicy::Manifest, &overrides).unwrap();

        // The computed matrix advances the minor component; the manifest arm is
        // bypassed by the explicit operator override.
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 2, 0)));
        assert_eq!(entries[0].winning_input, BumpSource::Argv);
    }

    #[test]
    fn pre_conflicts_with_the_manifest_policy() {
        let overrides = BumpOverrides::new().with_prerelease("rc");

        assert!(
            seed_plan(
                "0.1.0-alpha.2",
                Some("0.1.0-alpha.1"),
                BumpPolicy::Manifest,
                &overrides,
            )
            .is_err()
        );
    }

    #[test]
    fn a_tagless_manifest_module_cuts_its_declared_initial_release() {
        let entries = seed_plan(
            "0.1.0-alpha.1",
            None,
            BumpPolicy::Manifest,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(
            entries[0].planned_version,
            Some(Version::parse("0.1.0-alpha.1").unwrap())
        );
        // A never-released module always joins as an initial release, so the
        // manifest policy is a consistent no-op there.
        assert_eq!(entries[0].reason, BumpReason::InitialRelease);
    }

    #[test]
    fn semver_cascade_default_finalizes_a_pending_prerelease_unchanged() {
        // Regression: the default policy path is untouched — a patch of a pending
        // prerelease still finalizes it to its release.
        let entries = seed_plan(
            "0.1.0-alpha.1",
            Some("0.1.0-alpha.1"),
            BumpPolicy::SemverCascade,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 0)));
        assert_eq!(entries[0].reason, BumpReason::Changed);
    }

    #[test]
    fn semver_cascade_patches_a_manifest_at_its_baseline() {
        // Single-phase case: the manifest declares exactly the released baseline
        // (0.1.0) and has changed since, so a patch advances it to 0.1.1. The
        // baseline-anchored increment reduces to the plain patch here.
        let entries = seed_plan(
            "0.1.0",
            Some("0.1.0"),
            BumpPolicy::SemverCascade,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 1)));
        assert_eq!(entries[0].reason, BumpReason::Changed);
    }

    #[test]
    fn semver_cascade_cuts_an_already_resolved_manifest_without_double_bumping() {
        // Regression for the `bump -> PR -> merge -> tag` flow: a stage-only
        // bump advanced the manifest to 0.1.1 and it merged, but `release tag`
        // still sees the module changed since the 0.1.0 tag. The semver-cascade
        // increment anchors on the released baseline, so tag cuts the
        // already-resolved 0.1.1 rather than recomputing a second patch to 0.1.2.
        let entries = seed_plan(
            "0.1.1",
            Some("0.1.0"),
            BumpPolicy::SemverCascade,
            &BumpOverrides::new(),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 1, 1)));
    }

    #[test]
    fn semver_cascade_escalates_from_the_baseline_over_an_already_resolved_patch() {
        // A patch (0.1.1) was already resolved and merged, then a breaking change
        // landed. The increment anchors on the 0.1.0 baseline and lifts to a
        // minor (0.2.0), which wins over the lower already-resolved 0.1.1 — the
        // cut is `max(current, next(baseline, level))`, not a blind cut of the
        // declared version.
        let core = core_module();
        let key = core.key();
        let graph = Graph::build(vec![core.clone()], Vec::new()).unwrap();

        let mut targets = ReleaseTargets::new();
        targets.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(FakeReleaseTarget::new().with_declared_version(Version::new(0, 1, 1)))
                as Box<dyn ReleaseAdapter>,
        );

        let mut settings = BTreeMap::new();
        settings.insert(key.clone(), settings_for(&ReleaseConfig::default()));

        let mut changelogs = BTreeMap::new();
        changelogs.insert(
            key.clone(),
            ChangelogEntry::new(key.clone(), "breaking change", Vec::new()).with_breaking(true),
        );

        let mut baselines = BTreeMap::new();
        baselines.insert(
            key.clone(),
            ReleaseBaseline::tag(
                key.clone(),
                "rust/core@0.1.0",
                Version::new(0, 1, 0),
                Oid::new("cafe"),
            ),
        );

        let changed: BTreeSet<_> = std::iter::once(key).collect();
        let modules = vec![core];
        let overrides = BumpOverrides::new();

        let entries = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            changed: &changed,
            baselines: &baselines,
            changelogs: &changelogs,
            settings: &settings,
            targets: &targets,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        })
        .unwrap();

        assert_eq!(entries[0].level, BumpLevel::Minor);
        assert_eq!(entries[0].planned_version, Some(Version::new(0, 2, 0)));
    }
}
