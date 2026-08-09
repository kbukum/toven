//! Release bump planning: resolve each module's independent bump from config
//! and the per-run argv overrides, cascade dependency floors, and pre-skip
//! versions already satisfied by the registry (or, offline, the release tag).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use toven_model::{DepKind, Edge, Graph, MemberId, Module, ModuleKey, ModuleRef};
use toven_ports::{
    BumpLevel, DependentVersion, PublicationPolicy, ReleaseAdapter, ReleaseMutation, TagSigner,
};

use crate::model::tag;
use crate::versioning::strategy::{self, EffectiveLevel};
use crate::{
    BumpOverrides, BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseBaseline,
    ReleaseEntry, ResolvedReleaseSettings,
};

/// Inputs required to build release entries.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct BumpInputs<'a> {
    pub(crate) graph: &'a Graph,
    pub(crate) modules: &'a [Module],
    pub(crate) edges: &'a [Edge],
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

/// Whether a bump plan is a read-only projection (`release plan` and the other
/// previews), the standalone `release bump` mutation, or the cut a
/// verify-and-publish run (`release tag`/`publish`) will apply.
///
/// Two axes differ across the three intents:
/// - **manifest floor.** A projection reports a manifest version that is not
///   ahead of its released baseline as nothing-to-release; a `bump` likewise
///   drops it (there is nothing to advance); only a verify run fails closed so
///   it never re-cuts an already-released version.
/// - **maintainer-owned reach.** `plan`/`publish` force-include every
///   maintainer-owned module to verify its out-of-band tag; `bump` must not —
///   a maintainer-owned module whose manifest is not ahead of its baseline has
///   nothing to bump, so it stays out of the bump set.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum CutIntent {
    /// A read-only projection: a not-ahead manifest version is a no-op, not an
    /// error.
    Preview,
    /// The standalone `release bump` mutation: a not-ahead manifest version is a
    /// no-op (nothing to advance), and maintainer-owned modules are not
    /// force-included.
    Bump,
    /// A verify-and-publish cut (`release tag`/`publish`): a not-ahead manifest
    /// version fails closed, and maintainer-owned modules are force-included to
    /// verify their tags.
    Verify,
}

impl CutIntent {
    /// Whether a manifest version that is not ahead of its released baseline
    /// fails the run closed (`Verify`) rather than resolving to
    /// nothing-to-release (`Preview`/`Bump`).
    const fn not_ahead_is_fatal(self) -> bool {
        matches!(self, Self::Verify)
    }

    /// Whether change detection force-includes every maintainer-owned module to
    /// verify its out-of-band tag. Only the verify-and-publish path does; a
    /// `bump` reaches a maintainer-owned module only when it genuinely changed.
    pub(crate) const fn forces_maintainer_owned(self) -> bool {
        matches!(self, Self::Verify)
    }
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
#[allow(clippy::too_many_lines)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn plan_entries(input: &BumpInputs<'_>) -> AppResult<Vec<ReleaseEntry>> {
    let active = input.graph.closure(input.changed, release_closure_edge)?;
    let module_by_ref = input
        .modules
        .iter()
        .map(|module| (module.key(), module))
        .collect::<BTreeMap<_, _>>();

    input
        .overrides
        .validate_known(&active.iter().map(|key| key.module.clone()).collect())?;

    // `--pre` composes with the `semver-cascade` matrix only; under `manifest`
    // the prerelease channel already lives in the declared version, so a `--pre`
    // override is a contradictory usage that must fail closed rather than be
    // silently ignored.
    if input.policy == BumpPolicy::Manifest && input.overrides.prerelease().is_some() {
        return Err(AppError::invalid_input(
            "release.strategy",
            "strategy = \"manifest\": --pre conflicts with the manifest policy; \
             the prerelease channel is part of the declared manifest version",
        ));
    }

    let ranks = publish_ranks(input.graph, &active)?;
    let mut ordered = active.iter().cloned().collect::<Vec<_>>();
    ordered.sort_by_key(|module| (*ranks.get(module).unwrap_or(&usize::MAX), module.clone()));

    let mut planned_versions: BTreeMap<ModuleKey, Version> = BTreeMap::new();
    let mut cascade_roots: BTreeMap<ModuleKey, ModuleKey> = BTreeMap::new();
    let mut prepared = Vec::with_capacity(ordered.len());
    for reference in &ordered {
        let module = lookup(&module_by_ref, reference)?;
        let target = target_for(input.targets, module)?;
        let current = target.declared_version(module)?;
        // Every dependency has a lower topo rank, so its bump (if any) is already
        // recorded: a non-empty floor set means a direct dependency really bumped.
        let dep_floor_updates = dep_floor_updates(reference, input.edges, &planned_versions);
        // Attribute the cascade to the changed root carried forward by the actual
        // bumped direct dependency, not an arbitrary changed transitive ancestor.
        let origin = if input.changed.contains(reference) {
            cascade_roots.insert(reference.clone(), reference.clone());
            None
        } else {
            let root = triggering_dependency(reference, input.edges, &planned_versions)
                .and_then(|dependency| cascade_roots.get(&dependency).cloned());
            if let Some(root) = &root {
                cascade_roots.insert(reference.clone(), root.clone());
            }
            root
        };
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
        // A module pulled into the release closure with neither an own-version bump nor
        // a dependency floor to raise carries no mutation, so it must not reach APPLY
        // (which would rewrite manifests and cut a tag for nothing).
        if decision.planned.is_none() && dep_floor_updates.is_empty() {
            continue;
        }
        // An excluded module never participates in the release: no version change, no
        // tag, no target call, no hosted release. It is dropped before an entry exists.
        let publication = input
            .settings
            .get(&reference)
            .map_or(PublicationPolicy::TagOnly, |resolved| {
                resolved.publication.clone()
            });
        if !publication.releases() {
            continue;
        }
        let module = lookup(&module_by_ref, &reference)?;
        let target = target_for(input.targets, module)?;
        let (up_to_date, registry_publish_needed) =
            idempotency(input, module, target, &reference, decision.planned.as_ref());
        // Only registry-published modules invoke the publish loop; a tag-only module
        // is still versioned and tagged but never packaged/published to a registry.
        let publish_needed = registry_publish_needed && publication.publishes_to_registry();
        let cascade_origin = origin.filter(|_| decision.reason == BumpReason::DependencyCascade);
        let tag_format = input
            .settings
            .get(&reference)
            .and_then(|resolved| resolved.tag_format.clone());
        // Resolve the planned tag now so the plan explains the exact tag a
        // mutating run would create — and so a tag-scheme failure surfaces at
        // plan time rather than mid-mutation.
        let planned_tag = decision
            .planned
            .as_ref()
            .map(|version| {
                target
                    .tag_scheme(module, tag_format.as_deref())
                    .map(|scheme| tag::format(&scheme, version))
            })
            .transpose()?;
        let dep_floor_import_updates = dep_floor_updates
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
            new_version: decision.planned.clone(),
            dep_floor_updates,
            dep_floor_import_updates,
        };
        entries.push(ReleaseEntry {
            module: reference.clone(),
            current_version: current,
            planned_version: decision.planned,
            planned_tag,
            level: decision.level,
            reason: decision.reason,
            winning_input: decision.winning_input,
            cascade_origin,
            prerelease_channel: decision.prerelease_channel,
            up_to_date,
            mutation,
            publication,
            publish_needed,
            tag_format,
            tag_message: input
                .settings
                .get(&reference)
                .and_then(|resolved| resolved.tag_message.clone()),
            signer: input
                .settings
                .get(&reference)
                .filter(|resolved| resolved.sign_tags)
                .map(|resolved| TagSigner {
                    format: resolved.sign_format,
                    key: resolved.signing_key.clone(),
                }),
            commit_message: input
                .settings
                .get(&reference)
                .and_then(|resolved| resolved.commit_message.clone()),
            token_env: input
                .settings
                .get(&reference)
                .and_then(|resolved| resolved.token_env.clone()),
            visibility: input
                .settings
                .get(&reference)
                .map_or_else(Default::default, |resolved| resolved.visibility),
            push: input
                .settings
                .get(&reference)
                .map_or(PushPolicy::BranchAndTags, |resolved| resolved.push),
            remote: input
                .settings
                .get(&reference)
                .map_or_else(|| "origin".to_string(), |resolved| resolved.remote.clone()),
            branches: input
                .settings
                .get(&reference)
                .map_or_else(Vec::new, |resolved| resolved.branches.clone()),
            topo_rank: *ranks.get(&reference).unwrap_or(&usize::MAX),
            baseline: input.baselines.get(&reference).cloned(),
            changelog: input
                .changelogs
                .get(&reference)
                .cloned()
                .unwrap_or_else(|| {
                    ChangelogEntry::new(reference.clone(), "dependency cascade", Vec::new())
                }),
            changelog_path: input
                .settings
                .get(&reference)
                .and_then(|resolved| resolved.changelog.path.clone())
                .unwrap_or_else(|| "CHANGELOG.md".to_string()),
            changelog_roll: input
                .settings
                .get(&reference)
                .is_some_and(|resolved| resolved.changelog.roll),
            entrypoint: input
                .settings
                .get(&reference)
                .map_or_else(Default::default, |resolved| resolved.entrypoint),
            umbrella: input
                .settings
                .get(&reference)
                .is_some_and(|resolved| resolved.umbrella),
            version_references: input
                .settings
                .get(&reference)
                .map_or_else(Vec::new, |resolved| resolved.version_references.clone()),
        });
    }
    entries.sort_by(|left, right| {
        left.topo_rank
            .cmp(&right.topo_rank)
            .then_with(|| left.module.cmp(&right.module))
    });
    Ok(entries)
}

/// Resolve the bump decision for a maintainer-owned module: plan exactly the
/// version its manifest already declares against the tag/Release a maintainer
/// created out of band (the version decision already merged through `release
/// bump`). APPLY verifies the maintainer's tag matches this version and publishes,
/// and registry idempotency decides whether that publish is still needed.
///
/// Guarded against regressing below the released baseline: a maintainer-owned
/// module must declare the released version or newer. `current == baseline` is
/// allowed — that is the steady state a maintainer-owned re-run verifies and
/// republishes idempotently — but `current < baseline` means the manifest was
/// left behind the latest release, which would publish an older semver version
/// than already shipped. That fails closed under [`CutIntent::Verify`] and drops
/// from a [`CutIntent::Preview`] projection (nothing safely releasable), exactly
/// like the `manifest` policy's baseline floor.
///
/// # Errors
/// Fails closed under [`CutIntent::Verify`] when the declared version is behind
/// the released baseline.
fn maintainer_decision(
    input: &BumpInputs<'_>,
    reference: &ModuleKey,
    current: &Version,
) -> AppResult<BumpDecision> {
    let baseline = input
        .baselines
        .get(reference)
        .and_then(|b| b.version.as_ref());
    if let Some(base) = baseline
        && current < base
    {
        if input.intent.not_ahead_is_fatal() {
            return Err(AppError::invalid_input(
                "release.entrypoint",
                format!(
                    "maintainer-owned module '{}' declares {current}, behind the released \
                         baseline {base}; a maintainer-owned release never republishes a version \
                         below the latest release. Bump the manifest to the released version or \
                         newer before publishing.",
                    reference.module
                ),
            ));
        }
        // A projection or a `bump` treats a manifest behind its baseline as
        // nothing safely releasable: no planned version, so the entry drops.
        return Ok(BumpDecision {
            level: BumpLevel::Patch,
            planned: None,
            reason: BumpReason::Manifest,
            winning_input: BumpSource::Manifest,
            prerelease_channel: None,
        });
    }
    Ok(BumpDecision {
        level: classify(baseline.unwrap_or(&Version::new(0, 0, 0)), current),
        planned: Some(current.clone()),
        reason: BumpReason::Manifest,
        winning_input: BumpSource::Manifest,
        prerelease_channel: None,
    })
}

/// Resolve one module's own-version bump under the documented precedence (argv
/// `--set-version` > argv level > config level > adapter default), then a
/// dependency cascade for a dependent that did not itself change.
#[allow(clippy::too_many_lines)] // a linear walk through the documented bump precedence
fn resolve_bump(
    input: &BumpInputs<'_>,
    reference: &ModuleKey,
    current: &Version,
    is_cascade: bool,
) -> AppResult<BumpDecision> {
    let settings = input.settings.get(reference);
    let module_ref = &reference.module;

    // A maintainer-owned module keeps the version its manifest already declares
    // (see `maintainer_decision`); detection, the matrix, and cascades never move
    // it. The decision is guarded against regressing below the released baseline.
    if settings.is_some_and(|resolved| resolved.entrypoint.is_maintainer_owned()) {
        return maintainer_decision(input, reference, current);
    }

    if let Some(version) = input.overrides.set_version(module_ref) {
        if version <= current {
            return Err(AppError::invalid_input(
                "release.bump",
                format!(
                    "--set-version for module '{module_ref}' must exceed the current version {current} (got {version})"
                ),
            ));
        }
        return Ok(BumpDecision {
            level: classify(current, version),
            planned: Some(version.clone()),
            reason: BumpReason::Explicit,
            winning_input: BumpSource::SetVersion,
            prerelease_channel: None,
        });
    }

    let is_seed = input.changed.contains(reference);

    // The prerelease channel is only consulted for a module that cuts its own
    // version, so it is resolved here for a changed seed and lazily below for a
    // cascaded own-bump; a floor-only dependent never resolves it and so never
    // fails on a channel it would not use. An explicit `--pre` wins; otherwise a
    // configured branch→channel mapping selects the channel from the checked-out
    // branch.
    let seed_channel = if is_seed {
        effective_channel(input, settings, reference)?
    } else {
        None
    };

    // A module that has never been released cuts the version it already
    // declares: bumping past it would publish a version nobody declared and
    // would leave the declared version permanently unreleased. Explicit argv
    // (`--set-version`, handled above, `--patch`/`--minor`/`--major`, `--pre`)
    // or a branch-mapped prerelease channel still wins, so a deliberate first
    // bump stays possible.
    let is_initial = input
        .baselines
        .get(reference)
        .is_some_and(ReleaseBaseline::is_initial);
    if is_initial
        && is_seed
        && input.overrides.module_level(module_ref).is_none()
        && seed_channel.is_none()
    {
        return Ok(BumpDecision {
            planned: Some(current.clone()),
            level: classify(&Version::new(0, 0, 0), current),
            reason: BumpReason::InitialRelease,
            winning_input: BumpSource::Default,
            prerelease_channel: None,
        });
    }

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

    // A changed seed already resolved its channel above; a cascaded own-bump
    // resolves it now. A floor-only dependent returned above and never reaches
    // here, so it still never fails on a channel it would not use.
    let channel = if is_seed {
        seed_channel
    } else {
        effective_channel(input, settings, reference)?
    };

    // The `manifest` policy declares its own version rather than computing one
    // from the matrix. An explicit argv level override (`--patch`/`--minor`/
    // `--major`) still wins and takes the computed path even under `manifest`.
    if input.policy == BumpPolicy::Manifest && input.overrides.module_level(module_ref).is_none() {
        let baseline = input
            .baselines
            .get(reference)
            .and_then(|b| b.version.as_ref());
        let target = manifest_target(module_ref, current, baseline, input.intent)?;
        return Ok(target.map_or_else(
            // A preview whose declared version is not ahead of the baseline: no
            // own-version bump, so the module drops out of the plan (nothing to
            // release) unless a dependency floor still pulls it in.
            || BumpDecision {
                level: BumpLevel::Patch,
                planned: None,
                reason: BumpReason::Manifest,
                winning_input: BumpSource::Manifest,
                prerelease_channel: None,
            },
            |planned| BumpDecision {
                level: classify(baseline.unwrap_or(&Version::new(0, 0, 0)), &planned),
                planned: Some(planned),
                reason: BumpReason::Manifest,
                winning_input: BumpSource::Manifest,
                prerelease_channel: None,
            },
        ));
    }

    // Computed path: the semver matrix. Reached under `semver-cascade`, or under
    // `manifest` only when an explicit argv level override forced it — either
    // way the matrix advances the resolved component, never the (guarded)
    // manifest arm.
    let planned = strategy::next_version(
        BumpPolicy::SemverCascade,
        current,
        level,
        channel.as_deref(),
    )?;
    Ok(BumpDecision {
        planned: Some(planned),
        level: effective_to_level(level),
        reason,
        winning_input,
        prerelease_channel: channel,
    })
}

/// Resolve the version the `manifest` policy cuts: exactly the version the
/// manifest declares (`current`), guarded to be strictly ahead of the released
/// `baseline` under semver precedence.
///
/// A module with no baseline (never released) has no floor, so its declared
/// version is always cut. When the declared version is at or below the released
/// baseline there is nothing to cut, and the guard resolves by `intent`: a
/// [`CutIntent::Preview`] reports `None` (nothing to release) so the projection
/// stays safe to run anywhere, while a [`CutIntent::Verify`] fails closed so a
/// run never re-cuts an already-released version.
///
/// # Errors
/// Fails closed under [`CutIntent::Verify`] when the declared version is at or
/// below the released baseline, with an actionable message telling the operator
/// to bump the manifest first.
fn manifest_target(
    module_ref: &ModuleRef,
    current: &Version,
    baseline: Option<&Version>,
    intent: CutIntent,
) -> AppResult<Option<Version>> {
    match baseline {
        Some(base) if current <= base => {
            if intent.not_ahead_is_fatal() {
                Err(AppError::invalid_input(
                    "release.strategy",
                    format!(
                        "strategy = \"manifest\": module '{module_ref}' declares {current}, not \
                         ahead of the released baseline {base}. Bump the manifest version before \
                         releasing."
                    ),
                ))
            } else {
                // A projection or a `bump` reports nothing to release.
                Ok(None)
            }
        }
        _ => Ok(Some(current.clone())),
    }
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

/// Resolve the per-run prerelease channel for a module cutting an own version.
///
/// An explicit `--pre <channel>` argv wins and is validated against the
/// module's configured channels. Otherwise, when a branch→channel mapping is
/// configured, the module's member's checked-out branch selects the channel;
/// a detached HEAD or an unmapped branch yields a stable release. A mapped
/// channel is validated against the configured channels defensively (config
/// validation already enforces this), so a malformed mapping fails closed
/// rather than cutting an unrecognized prerelease.
///
/// # Errors
/// Rejects a `--pre` channel or a branch-mapped channel that is not one of the
/// module's configured prerelease channels.
fn effective_channel(
    input: &BumpInputs<'_>,
    settings: Option<&ResolvedReleaseSettings>,
    reference: &ModuleKey,
) -> AppResult<Option<String>> {
    if let Some(channel) = input.overrides.prerelease() {
        if !settings.is_some_and(|resolved| resolved.prerelease.recognizes(channel)) {
            return Err(AppError::invalid_input(
                "release.pre",
                format!("prerelease channel '{channel}' is not one of the configured channels"),
            ));
        }
        return Ok(Some(channel.to_string()));
    }
    let Some(resolved) = settings else {
        return Ok(None);
    };
    if resolved.prerelease.branch_channels.is_empty() {
        return Ok(None);
    }
    let Some(branch) = input.branches.get(&reference.member) else {
        return Ok(None);
    };
    let Some(channel) = resolved.prerelease.branch_channels.get(branch) else {
        return Ok(None);
    };
    if !resolved.prerelease.recognizes(channel) {
        return Err(AppError::invalid_input(
            "release.prerelease.branch_channels",
            format!(
                "branch '{branch}' maps to prerelease channel '{channel}', which is not one of \
                 the configured channels"
            ),
        ));
    }
    Ok(Some(channel.clone()))
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

/// Decide `(up_to_date, publish_needed)` for a planned version, anchoring on
/// the registry's published set (or, offline, the baseline release tag).
fn idempotency(
    input: &BumpInputs<'_>,
    module: &Module,
    target: &dyn ReleaseAdapter,
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
    // `published_versions` is best-effort: a transient registry/search failure must
    // not abort planning. Treat a lookup error as "publish needed" — the APPLY
    // publish loop's `AlreadyPublished` classification is the authoritative
    // idempotency backstop.
    let Ok(published) = target.published_versions(module) else {
        return (false, true);
    };
    let up_to_date = published.iter().max().is_some_and(|max| planned <= max);
    (up_to_date, !up_to_date)
}

/// The bumped direct dependency that triggers `module`'s cascade, chosen
/// deterministically (lowest key) when several same-ecosystem dependencies
/// bump.
///
/// Only same-member, same-ecosystem, non-overlay dependencies raise a floor, so
/// they are the only edges that can propagate a cascade. Because planning runs
/// dependency-first, every candidate's own bump is already recorded in
/// `planned_versions` by the time a dependent is processed.
fn triggering_dependency(
    module: &ModuleKey,
    edges: &[Edge],
    planned_versions: &BTreeMap<ModuleKey, Version>,
) -> Option<ModuleKey> {
    edges
        .iter()
        .filter(|edge| {
            &edge.from == module
                && edge.from.module.ecosystem == edge.to.module.ecosystem
                && edge.from.member == edge.to.member
                && !matches!(edge.kind, DepKind::Overlay)
                && planned_versions.contains_key(&edge.to)
        })
        .map(|edge| edge.to.clone())
        .min()
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
    targets: &'a crate::ReleaseTargets,
    module: &Module,
) -> AppResult<&'a dyn ReleaseAdapter> {
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
    use rskit_errors::AppResult;
    use rskit_version::semver::Version;
    use toven_model::{DepKind, EcosystemId, Edge, Graph, MemberId, Module, RepoPath};
    use toven_ports::{BumpLevel, DependentVersion, Oid, ReleaseAdapter, ReleaseConfig};
    use toven_testkit::FakeReleaseTarget;

    use super::{
        BTreeMap, BTreeSet, BumpInputs, BumpOverrides, BumpPolicy, BumpReason, BumpSource,
        ChangelogEntry, CutIntent, ModuleRef, ReleaseBaseline, ReleaseEntry,
        ResolvedReleaseSettings, plan_entries,
    };
    use crate::ReleaseTargets;

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
        let edges = Vec::new();

        let result = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            edges: &edges,
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
        let edges = Vec::new();
        let overrides = BumpOverrides::new();

        let error = plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            edges: &edges,
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
        let graph = Graph::build(modules.clone(), edges.clone()).unwrap();

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
        let edges = Vec::new();

        plan_entries(&BumpInputs {
            graph: &graph,
            modules: &modules,
            edges: &edges,
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
}
