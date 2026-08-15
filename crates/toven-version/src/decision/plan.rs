//! The single pure version decision: `plan_bumps`.
//!
//! Resolves each module's independent bump from pre-gathered
//! [`VersionInputs`] and the run-wide [`BumpConfig`], cascades dependency
//! floors, and pre-skips versions already satisfied by the registry (or,
//! offline, the release tag). It is **pure**: values in, a [`BumpPlan`] out — no
//! `VcsReader`, no ecosystem adapter, no I/O. Baseline anchoring is an input
//! field ([`VersionInputs::baseline`]), so the two decision bugs it once hid
//! (an umbrella anchor using the wrong version; a maintainer echo skipping
//! change-gating) are properties of this function, covered by git-free tests.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use toven_model::{DepKind, Edge, Graph, ModuleKey, ModuleRef};
use toven_ports::{BumpLevel, DependentVersion};

use toven_semver::{EffectiveLevel, next_version};

use crate::baseline::ReleaseBaseline;
use crate::policy::{BumpPolicy, BumpReason, BumpSource};

use super::inputs::{BumpConfig, CutIntent, ModuleVersionConfig, VersionInputs};

/// One module's resolved bump decision, before entry assembly.
///
/// The pure decision fields `toven-release` composes into a `ReleaseEntry` with
/// its resolved settings, tag formatting, and mutation import mapping.
#[derive(Debug, Clone)]
pub struct BumpEntry {
    /// Module being considered for release.
    pub module: ModuleKey,
    /// Version the module's manifest currently declares.
    pub current_version: Version,
    /// Version to release, if this module receives an own-version bump.
    pub planned_version: Option<Version>,
    /// The effective bump level applied to reach the planned version.
    pub level: BumpLevel,
    /// Why this module is being bumped.
    pub reason: BumpReason,
    /// Which input won under the documented precedence.
    pub winning_input: BumpSource,
    /// The changed module that triggered this cascade, when `reason` is a
    /// dependency cascade.
    pub cascade_origin: Option<ModuleKey>,
    /// Prerelease channel applied to the planned version, when cutting a
    /// prerelease.
    pub prerelease_channel: Option<String>,
    /// Whether the planned version is already at/above the registry (or,
    /// offline, the release tag), making a real publish a reported no-op.
    pub up_to_date: bool,
    /// Whether the publish loop must publish this module/version.
    pub publish_needed: bool,
    /// Dependency-floor version bumps to apply to this module's manifest, keyed
    /// by the dependency's module ref.
    pub dep_floor_updates: BTreeMap<ModuleRef, Version>,
    /// Topological rank used for deterministic publish ordering.
    pub topo_rank: usize,
    /// Baseline used for change detection.
    pub baseline: ReleaseBaseline,
}

/// The pure result of a version decision: the per-module bump entries in
/// deterministic publish order.
#[derive(Debug, Clone)]
pub struct BumpPlan {
    /// Per-module bump decisions, already sorted in deterministic publish order.
    pub entries: Vec<BumpEntry>,
}

/// One module's pure incremental resolution.
#[derive(Debug, Clone)]
pub struct BumpResolution {
    /// Module that was resolved.
    pub module: ModuleKey,
    /// Version currently declared by the module.
    pub current_version: Version,
    /// Planned release entry, absent when the module has no release work.
    pub entry: Option<BumpEntry>,
}

/// Stateful pure planner for a dependency-first module stream.
///
/// The caller gathers one module's [`VersionInputs`] at a time in
/// [`Self::modules`] order and passes it to [`Self::decide`]. The planner owns
/// cascade state only; it performs no I/O and emits no events.
pub struct BumpPlanner<'a> {
    decision: Decision<'a>,
    ordered: Vec<ModuleKey>,
    ranks: BTreeMap<ModuleKey, usize>,
    next: usize,
    planned_versions: BTreeMap<ModuleKey, Version>,
    cascade_roots: BTreeMap<ModuleKey, ModuleKey>,
    active: BTreeSet<ModuleKey>,
    entries: Vec<BumpEntry>,
}

/// The resolved own-version bump for one module, before idempotency pre-skip.
struct BumpDecision {
    planned: Option<Version>,
    level: BumpLevel,
    reason: BumpReason,
    winning_input: BumpSource,
    prerelease_channel: Option<String>,
}

/// The gathered inputs indexed for the incremental decision walk.
struct Decision<'a> {
    cfg: &'a BumpConfig<'a>,
    inputs: BTreeMap<ModuleKey, VersionInputs>,
    changed: BTreeSet<ModuleKey>,
}

impl<'a> Decision<'a> {
    const fn new(cfg: &'a BumpConfig<'a>) -> Self {
        Self {
            cfg,
            inputs: BTreeMap::new(),
            changed: BTreeSet::new(),
        }
    }

    fn insert(&mut self, input: VersionInputs) {
        if input.changed {
            self.changed.insert(input.module.clone());
        }
        self.inputs.insert(input.module.clone(), input);
    }

    fn input(&self, reference: &ModuleKey) -> AppResult<&VersionInputs> {
        self.inputs.get(reference).ok_or_else(|| {
            AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
        })
    }

    fn config(&self, reference: &ModuleKey) -> Option<&ModuleVersionConfig> {
        self.inputs.get(reference).map(|input| &input.config)
    }

    fn baseline(&self, reference: &ModuleKey) -> Option<&ReleaseBaseline> {
        self.inputs.get(reference).map(|input| &input.baseline)
    }

    fn breaking(&self, reference: &ModuleKey) -> bool {
        self.inputs
            .get(reference)
            .is_some_and(|input| input.breaking)
    }
}

impl<'a> BumpPlanner<'a> {
    /// Create a pure planner over the modules that this decision pass will
    /// examine.
    ///
    /// Modules are ordered dependency-first with deterministic key ordering
    /// inside each ready wave. The same order drives I/O in `toven-release` and
    /// the incremental cascade here.
    ///
    /// # Errors
    /// Rejects duplicate or unknown module keys, invalid overrides, a graph
    /// failure, or a manifest-policy/prerelease conflict.
    pub fn new(
        modules: impl IntoIterator<Item = ModuleKey>,
        cfg: &'a BumpConfig<'a>,
    ) -> AppResult<Self> {
        let mut scope = BTreeSet::new();
        for module in modules {
            if !cfg.graph.contains(&module) {
                return Err(AppError::invalid_input(
                    "release.modules",
                    format!("unknown module '{module}'"),
                ));
            }
            if !scope.insert(module.clone()) {
                return Err(AppError::invalid_input(
                    "release.modules",
                    format!("duplicate versioning input for module '{module}'"),
                ));
            }
        }
        cfg.overrides
            .validate_known(&scope.iter().map(|key| key.module.clone()).collect())?;
        if cfg.policy == BumpPolicy::Manifest && cfg.overrides.prerelease().is_some() {
            return Err(AppError::invalid_input(
                "release.strategy",
                "strategy = \"manifest\": --pre conflicts with the manifest policy; \
                 the prerelease channel is part of the declared manifest version",
            ));
        }

        let ranks = publish_ranks(cfg.graph, &scope)?;
        let mut ordered = scope.into_iter().collect::<Vec<_>>();
        ordered.sort_by_key(|module| (*ranks.get(module).unwrap_or(&usize::MAX), module.clone()));
        Ok(Self {
            decision: Decision::new(cfg),
            ordered,
            ranks,
            next: 0,
            planned_versions: BTreeMap::new(),
            cascade_roots: BTreeMap::new(),
            active: BTreeSet::new(),
            entries: Vec::new(),
        })
    }

    /// Modules to gather and decide, in dependency-first publication order.
    #[must_use]
    pub fn modules(&self) -> &[ModuleKey] {
        &self.ordered
    }

    /// Decide one gathered module and advance the cascade state.
    ///
    /// # Errors
    /// Rejects an out-of-order input or propagates invalid bump, prerelease,
    /// manifest, and graph-policy combinations.
    #[allow(clippy::too_many_lines)]
    pub fn decide(&mut self, input: VersionInputs) -> AppResult<BumpResolution> {
        let expected = self.ordered.get(self.next).ok_or_else(|| {
            AppError::invalid_input(
                "release.modules",
                format!(
                    "unexpected extra versioning input for module '{}'",
                    input.module
                ),
            )
        })?;
        if input.module != *expected {
            return Err(AppError::invalid_input(
                "release.modules",
                format!(
                    "versioning input for module '{}' arrived out of order; expected '{expected}'",
                    input.module
                ),
            ));
        }
        let reference = input.module.clone();
        let current = input.current_version.clone();
        self.decision.insert(input);

        let dep_floor_updates = dep_floor_updates(
            &reference,
            self.decision.cfg.graph.edges(),
            &self.planned_versions,
        );
        let active = self.decision.changed.contains(&reference) || !dep_floor_updates.is_empty();
        if !active {
            self.next += 1;
            return Ok(BumpResolution {
                module: reference,
                current_version: current,
                entry: None,
            });
        }
        self.active.insert(reference.clone());

        let origin = if self.decision.changed.contains(&reference) {
            self.cascade_roots
                .insert(reference.clone(), reference.clone());
            None
        } else {
            let root = triggering_dependency(
                &reference,
                self.decision.cfg.graph.edges(),
                &self.planned_versions,
            )
            .and_then(|dependency| self.cascade_roots.get(&dependency).cloned());
            if let Some(root) = &root {
                self.cascade_roots.insert(reference.clone(), root.clone());
            }
            root
        };
        let bump = resolve_bump(
            &self.decision,
            &reference,
            &current,
            !dep_floor_updates.is_empty(),
        )?;
        if let Some(version) = &bump.planned {
            self.planned_versions
                .insert(reference.clone(), version.clone());
        }

        let publication = self
            .decision
            .config(&reference)
            .map(|config| &config.publication);
        let entry = if (bump.planned.is_none() && dep_floor_updates.is_empty())
            || !publication.is_none_or(toven_ports::PublicationPolicy::releases)
        {
            None
        } else {
            let input = self.decision.input(&reference)?;
            let (up_to_date, registry_publish_needed) =
                idempotency(&self.decision, &reference, bump.planned.as_ref());
            let publish_needed =
                registry_publish_needed && input.config.publication.publishes_to_registry();
            let cascade_origin = origin.filter(|_| bump.reason == BumpReason::DependencyCascade);
            Some(BumpEntry {
                module: reference.clone(),
                current_version: current.clone(),
                planned_version: bump.planned,
                level: bump.level,
                reason: bump.reason,
                winning_input: bump.winning_input,
                cascade_origin,
                prerelease_channel: bump.prerelease_channel,
                up_to_date,
                publish_needed,
                dep_floor_updates,
                topo_rank: *self.ranks.get(&reference).unwrap_or(&usize::MAX),
                baseline: input.baseline.clone(),
            })
        };
        if let Some(entry) = &entry {
            self.entries.push(entry.clone());
        }
        self.next += 1;
        Ok(BumpResolution {
            module: reference,
            current_version: current,
            entry,
        })
    }

    /// Finish the incremental pass and return its deterministic release plan.
    ///
    /// # Errors
    /// Fails when one or more configured modules were never decided.
    pub fn finish(self) -> AppResult<BumpPlan> {
        if self.next != self.ordered.len() {
            let missing = &self.ordered[self.next];
            return Err(AppError::invalid_input(
                "release.modules",
                format!("missing versioning input for module '{missing}'"),
            ));
        }
        self.decision.cfg.overrides.validate_known(
            &self
                .active
                .iter()
                .map(|key| key.module.clone())
                .collect(),
        )?;
        Ok(BumpPlan {
            entries: self.entries,
        })
    }
}

/// Reject duplicate module keys in the gathered inputs.
///
/// `inputs` is a public slice, so a caller could pass two entries for the same
/// module. The indexed view would silently keep whichever appeared last while
/// the changed set aggregates across every copy — conflicting duplicates could
/// then mark a module active yet plan its bump from an arbitrary current
/// version/baseline. Fail closed at the API boundary instead so malformed
/// gathered data can never produce an arbitrary plan.
fn reject_duplicate_inputs(inputs: &[VersionInputs]) -> AppResult<()> {
    let mut seen = BTreeSet::new();
    for input in inputs {
        if !seen.insert(&input.module) {
            return Err(AppError::invalid_input(
                "release.modules",
                format!("duplicate versioning input for module '{}'", input.module),
            ));
        }
    }
    Ok(())
}

/// Plan every module's bump from the pre-gathered inputs.
///
/// Bumps are decided **dependency-first** so a dependent only cascades when a
/// direct dependency actually receives an own-version bump — a dependent whose
/// dependencies stayed put (e.g. an `upgrade`-mode intermediate that raised a
/// floor without republishing) is never given a bump that carries no change.
/// Propagates an invalid `--set-version`/`--pre`/override combination, a
/// duplicate module key in `inputs`, an unknown module in the release closure, a
/// graph failure, or a prerelease channel that is not one of a module's
/// configured channels.
#[allow(clippy::too_many_lines)]
pub fn plan_bumps(inputs: &[VersionInputs], cfg: &BumpConfig<'_>) -> AppResult<BumpPlan> {
    reject_duplicate_inputs(inputs)?;
    let changed = inputs
        .iter()
        .filter(|input| input.changed)
        .map(|input| input.module.clone())
        .collect();
    let active = cfg.graph.closure(&changed, release_closure_edge)?;
    let mut planner = BumpPlanner::new(active, cfg)?;
    let indexed = inputs
        .iter()
        .map(|input| (input.module.clone(), input))
        .collect::<BTreeMap<_, _>>();
    for module in planner.modules().to_vec() {
        let input = indexed.get(&module).ok_or_else(|| {
            AppError::invalid_input("release.modules", format!("unknown module '{module}'"))
        })?;
        planner.decide((*input).clone())?;
    }
    planner.finish()
}

/// Resolve the bump decision for a maintainer-owned module: plan exactly the
/// version its manifest already declares against the tag/Release a maintainer
/// created out of band. APPLY verifies the maintainer's tag matches this version
/// and publishes, and registry idempotency decides whether that publish is still
/// needed.
///
/// Guarded against regressing below the released baseline: `current == baseline`
/// is allowed (the steady state a maintainer-owned re-run verifies), but
/// `current < baseline` fails closed under [`CutIntent::Verify`] and drops from
/// a [`CutIntent::Preview`]/[`CutIntent::Bump`] projection.
///
/// # Errors
/// Fails closed under [`CutIntent::Verify`] when the declared version is behind
/// the released baseline.
fn maintainer_decision(
    decision: &Decision<'_>,
    reference: &ModuleKey,
    current: &Version,
) -> AppResult<BumpDecision> {
    let baseline = decision
        .baseline(reference)
        .and_then(|b| b.version.as_ref());
    if let Some(base) = baseline
        && current < base
    {
        if decision.cfg.intent.not_ahead_is_fatal() {
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
    decision: &Decision<'_>,
    reference: &ModuleKey,
    current: &Version,
    is_cascade: bool,
) -> AppResult<BumpDecision> {
    let cfg = decision.cfg;
    let config = decision.config(reference);
    let module_ref = &reference.module;

    // On the verify-and-publish path a maintainer-owned module keeps the version
    // its manifest already declares (see `maintainer_decision`). `bump`/`plan` do
    // NOT freeze it: they compute the change-gated, cascaded increment the
    // maintainer will review and merge.
    if cfg.intent.verifies_maintainer_version()
        && config.is_some_and(|config| config.entrypoint.is_maintainer_owned())
    {
        return maintainer_decision(decision, reference, current);
    }

    if let Some(version) = cfg.overrides.set_version(module_ref) {
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

    let is_seed = decision.changed.contains(reference);

    // The prerelease channel is only consulted for a module that cuts its own
    // version, so it is resolved here for a changed seed and lazily below for a
    // cascaded own-bump; a floor-only dependent never resolves it and so never
    // fails on a channel it would not use.
    let seed_channel = if is_seed {
        effective_channel(cfg, config, reference)?
    } else {
        None
    };

    // A module that has never been released cuts the version it already
    // declares: bumping past it would publish a version nobody declared and
    // would leave the declared version permanently unreleased. Explicit argv or
    // a branch-mapped prerelease channel still wins.
    let is_initial = decision
        .baseline(reference)
        .is_some_and(ReleaseBaseline::is_initial);
    if is_initial
        && is_seed
        && cfg.overrides.module_level(module_ref).is_none()
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

    let breaking = decision.breaking(reference);
    let (level, winning_input, reason) =
        select_level(cfg, module_ref, config, is_seed, is_cascade, breaking);

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
        effective_channel(cfg, config, reference)?
    };

    // The `manifest` policy declares its own version rather than computing one
    // from the matrix. An explicit argv level override (`--patch`/`--minor`/
    // `--major`) still wins and takes the computed path even under `manifest`.
    if cfg.policy == BumpPolicy::Manifest && cfg.overrides.module_level(module_ref).is_none() {
        let baseline = decision
            .baseline(reference)
            .and_then(|b| b.version.as_ref());
        let target = manifest_target(module_ref, current, baseline, cfg.intent)?;
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

    // Computed path: the semver matrix. The increment anchors on the released
    // baseline, not the declared manifest, so a version a prior `release bump`
    // already resolved and merged is not advanced a second time; `max(current,
    // target)` cuts the already-resolved manifest version rather than
    // recomputing another increment. A module with no baseline (never released)
    // falls back to the declared version as its anchor.
    let anchor = decision
        .baseline(reference)
        .and_then(|baseline| baseline.version.as_ref())
        .unwrap_or(current);
    let target = next_version(anchor, level, channel.as_deref())?;
    let planned = target.max(current.clone());
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
    cfg: &BumpConfig<'_>,
    module_ref: &ModuleRef,
    config: Option<&ModuleVersionConfig>,
    is_seed: bool,
    is_cascade: bool,
    breaking: bool,
) -> (Option<EffectiveLevel>, BumpSource, BumpReason) {
    if let Some(level) = cfg.overrides.module_level(module_ref) {
        let reason = if is_seed {
            BumpReason::Changed
        } else {
            BumpReason::DependencyCascade
        };
        return (Some(level_to_effective(level)), BumpSource::Argv, reason);
    }
    if is_seed {
        return match config.map_or(BumpLevel::Auto, |config| config.level) {
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
        return match config.map_or(DependentVersion::Bump, |config| config.dependent_version) {
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
/// configured, the module's member's checked-out branch selects the channel; a
/// detached HEAD or an unmapped branch yields a stable release.
///
/// # Errors
/// Rejects a `--pre` channel or a branch-mapped channel that is not one of the
/// module's configured prerelease channels.
fn effective_channel(
    cfg: &BumpConfig<'_>,
    config: Option<&ModuleVersionConfig>,
    reference: &ModuleKey,
) -> AppResult<Option<String>> {
    if let Some(channel) = cfg.overrides.prerelease() {
        if !config.is_some_and(|config| config.prerelease.recognizes(channel)) {
            return Err(AppError::invalid_input(
                "release.pre",
                format!("prerelease channel '{channel}' is not one of the configured channels"),
            ));
        }
        return Ok(Some(channel.to_string()));
    }
    let Some(config) = config else {
        return Ok(None);
    };
    if config.prerelease.branch_channels.is_empty() {
        return Ok(None);
    }
    let Some(branch) = cfg.branches.get(&reference.member) else {
        return Ok(None);
    };
    let Some(channel) = config.prerelease.branch_channels.get(branch) else {
        return Ok(None);
    };
    if !config.prerelease.recognizes(channel) {
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
    decision: &Decision<'_>,
    reference: &ModuleKey,
    planned: Option<&Version>,
) -> (bool, bool) {
    let Some(planned) = planned else {
        // A floor-only upgrade never publishes an own version.
        return (false, false);
    };
    let config = decision.config(reference);
    let offline = decision.cfg.overrides.offline() || config.is_some_and(|config| config.offline);
    if offline {
        let up_to_date = decision
            .baseline(reference)
            .and_then(|baseline| baseline.version.as_ref())
            .is_some_and(|tagged| planned <= tagged);
        return (up_to_date, !up_to_date);
    }
    // The published set is empty when a lookup failed: treat that as "publish
    // needed" — the APPLY publish loop's `AlreadyPublished` classification is the
    // authoritative idempotency backstop.
    let published = decision
        .inputs
        .get(reference)
        .map(|input| input.published_versions.as_slice())
        .unwrap_or_default();
    if published.is_empty() {
        return (false, true);
    }
    let up_to_date = published.iter().max().is_some_and(|max| planned <= max);
    (up_to_date, !up_to_date)
}

/// The bumped direct dependency that triggers `module`'s cascade, chosen
/// deterministically (lowest key) when several same-ecosystem dependencies
/// bump.
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
    use std::collections::BTreeMap;

    use rskit_version::semver::Version;
    use toven_model::{
        DepKind, EcosystemId, Edge, Entrypoint, Graph, MemberId, Module, ModuleKey, ModuleRef,
        RepoPath,
    };
    use toven_ports::{BumpLevel, DependentVersion, PrereleaseConfig, PublicationPolicy};

    use crate::baseline::ReleaseBaseline;
    use crate::overrides::BumpOverrides;
    use crate::policy::{BumpPolicy, BumpReason, BumpSource};

    use super::super::inputs::{BumpConfig, CutIntent, ModuleVersionConfig, VersionInputs};
    use super::{BumpPlan, BumpPlanner, plan_bumps};

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("ecosystem"), name).expect("ref"),
            RepoPath::new(format!("crates/{name}")).expect("root"),
        )
    }

    fn key(name: &str) -> ModuleKey {
        module(name).key()
    }

    /// A default decision config: `auto` level, cascading dependents, tag-only
    /// publication (so the module releases but never invokes the publish loop),
    /// online, Toven-owned.
    fn config() -> ModuleVersionConfig {
        ModuleVersionConfig {
            level: BumpLevel::Auto,
            dependent_version: DependentVersion::Bump,
            prerelease: PrereleaseConfig::default(),
            publication: PublicationPolicy::TagOnly,
            offline: false,
            entrypoint: Entrypoint::Toven,
        }
    }

    /// A tag baseline anchored at `version` (a `cafe` commit), or an initial
    /// baseline when `version` is `None`.
    fn baseline(name: &str, version: Option<&str>) -> ReleaseBaseline {
        version.map_or_else(
            || ReleaseBaseline::initial(key(name)),
            |raw| {
                let parsed = Version::parse(raw).expect("version");
                ReleaseBaseline::tag(
                    key(name),
                    format!("rust/{name}@{parsed}"),
                    parsed,
                    toven_ports::Oid::new("cafe"),
                )
            },
        )
    }

    /// A changed seed input declaring `current`, anchored on `base`.
    fn seed(
        name: &str,
        current: &str,
        base: Option<&str>,
        config: ModuleVersionConfig,
    ) -> VersionInputs {
        VersionInputs {
            module: key(name),
            current_version: Version::parse(current).expect("current"),
            published_versions: Vec::new(),
            baseline: baseline(name, base),
            changed: true,
            breaking: false,
            config,
        }
    }

    fn no_branches() -> BTreeMap<Option<MemberId>, String> {
        BTreeMap::new()
    }

    /// Plan the given inputs over an isolated graph with no edges.
    fn plan_seed(inputs: &[VersionInputs], policy: BumpPolicy, intent: CutIntent) -> BumpPlan {
        let modules = inputs
            .iter()
            .map(|input| module(&input.module.module.name))
            .collect::<Vec<_>>();
        let graph = Graph::build(modules, Vec::new()).expect("graph");
        let overrides = BumpOverrides::new();
        plan_bumps(
            inputs,
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy,
                overrides: &overrides,
                intent,
            },
        )
        .expect("plan")
    }

    #[test]
    fn a_breaking_changelog_classification_forces_a_minor_bump() {
        let mut input = seed("core", "0.1.0", Some("0.1.0"), config());
        input.breaking = true;

        let plan = plan_seed(&[input], BumpPolicy::SemverCascade, CutIntent::Verify);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].level, BumpLevel::Minor);
        assert_eq!(plan.entries[0].winning_input, BumpSource::Changelog);
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 2, 0)));
    }

    #[test]
    fn duplicate_versioning_inputs_are_rejected_at_the_api_boundary() {
        // Two inputs for the same module with conflicting current versions: the
        // indexed view would silently keep the last while `changed` aggregates
        // across both, so the plan must fail closed rather than bump from an
        // arbitrary copy.
        let first = seed("core", "0.1.0", Some("0.1.0"), config());
        let mut second = seed("core", "0.9.0", Some("0.1.0"), config());
        second.changed = false;
        let graph = Graph::build(vec![module("core")], Vec::new()).expect("graph");
        let overrides = BumpOverrides::new();

        let error = plan_bumps(
            &[first, second],
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::SemverCascade,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        )
        .expect_err("duplicate inputs must be rejected");
        assert!(
            error.to_string().contains("duplicate versioning input"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_umbrella_anchored_independent_module_bumps_from_its_own_version_not_the_umbrella() {
        // The bug-1 regression: an umbrella workspace versions its modules
        // independently under one shared tag (v1.4.0), but `core` declares and
        // released 0.2.0. GATHER resolved the baseline to core's OWN version at
        // the umbrella commit (0.2.0), so the patch bump is 0.2.0 -> 0.2.1, never
        // an umbrella-anchored 1.4.x.
        let umbrella_base = ReleaseBaseline::anchored(
            key("core"),
            Some("v1.4.0".to_string()),
            Some(Version::new(0, 2, 0)),
            Some(toven_ports::Oid::new("umbrella")),
        );
        let input = VersionInputs {
            module: key("core"),
            current_version: Version::new(0, 2, 0),
            published_versions: Vec::new(),
            baseline: umbrella_base,
            changed: true,
            breaking: false,
            config: config(),
        };

        let plan = plan_seed(&[input], BumpPolicy::SemverCascade, CutIntent::Verify);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.entries[0].planned_version,
            Some(Version::new(0, 2, 1)),
            "the bump anchors on core's own 0.2.0, not the umbrella tag's 1.4.0"
        );
    }

    #[test]
    fn a_maintainer_owned_module_echoes_its_declared_version_on_the_verify_path() {
        let mut cfg = config();
        cfg.entrypoint = Entrypoint::Maintainer;
        let input = seed("core", "0.1.0", None, cfg);

        let plan = plan_seed(&[input], BumpPolicy::SemverCascade, CutIntent::Verify);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 0)));
        assert_eq!(plan.entries[0].reason, BumpReason::Manifest);
        assert_eq!(plan.entries[0].winning_input, BumpSource::Manifest);
    }

    #[test]
    fn a_maintainer_owned_module_computes_a_change_gated_bump_on_the_bump_path() {
        // The bug-2 regression: under `entrypoint = "maintainer"`, `release bump`
        // (CutIntent::Bump) must COMPUTE a change-gated increment, not echo the
        // declared version. The crate declares its released baseline (0.1.0) and
        // changed, so bump advances it to 0.1.1. Only Verify echoes.
        let mut cfg = config();
        cfg.entrypoint = Entrypoint::Maintainer;
        let input = seed("core", "0.1.0", Some("0.1.0"), cfg);

        let plan = plan_seed(&[input], BumpPolicy::SemverCascade, CutIntent::Bump);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.entries[0].planned_version,
            Some(Version::new(0, 1, 1)),
            "bump computes a real increment under maintainer entrypoint, not echo 0.1.0"
        );
        assert_eq!(plan.entries[0].reason, BumpReason::Changed);
    }

    #[test]
    fn a_maintainer_owned_module_fails_closed_when_declared_version_is_behind_the_baseline() {
        let mut cfg = config();
        cfg.entrypoint = Entrypoint::Maintainer;
        let input = seed("core", "0.1.0", Some("0.2.0"), cfg);

        let modules = vec![module("core")];
        let graph = Graph::build(modules, Vec::new()).expect("graph");
        let overrides = BumpOverrides::new();
        let result = plan_bumps(
            &[input],
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::SemverCascade,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn a_set_version_at_or_below_the_current_version_is_rejected() {
        let input = seed("core", "0.1.0", None, config());
        let modules = vec![module("core")];
        let graph = Graph::build(modules, Vec::new()).expect("graph");
        let overrides = BumpOverrides::new()
            .with_set_version(input.module.module.clone(), Version::new(0, 1, 0))
            .expect("override");

        let result = plan_bumps(
            &[input],
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::SemverCascade,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn a_tagless_seed_cuts_its_declared_initial_release() {
        let input = seed("core", "0.3.0", None, config());

        let plan = plan_seed(&[input], BumpPolicy::SemverCascade, CutIntent::Verify);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 3, 0)));
        assert_eq!(plan.entries[0].reason, BumpReason::InitialRelease);
    }

    fn dep_input(name: &str, dependent_version: DependentVersion) -> VersionInputs {
        let mut cfg = config();
        cfg.dependent_version = dependent_version;
        VersionInputs {
            module: key(name),
            current_version: Version::new(0, 1, 0),
            published_versions: Vec::new(),
            baseline: baseline(name, Some("0.1.0")),
            changed: false,
            breaking: false,
            config: cfg,
        }
    }

    #[test]
    fn an_upgrade_only_dependency_does_not_cascade_a_bump_to_its_dependents() {
        // app -> lib -> base; base changes, lib only raises floors (upgrade), so
        // app's direct dependency never republishes and app is dropped entirely.
        let base = {
            let mut input = dep_input("base", DependentVersion::Bump);
            input.changed = true;
            input
        };
        let lib = dep_input("lib", DependentVersion::Upgrade);
        let app = dep_input("app", DependentVersion::Bump);
        let (base_key, lib_key, app_key) = (key("base"), key("lib"), key("app"));
        let edges = vec![
            Edge::new(app_key.clone(), lib_key.clone(), DepKind::Normal),
            Edge::new(lib_key.clone(), base_key.clone(), DepKind::Normal),
        ];
        let graph =
            Graph::build(vec![module("base"), module("lib"), module("app")], edges).expect("graph");
        let overrides = BumpOverrides::new();
        let inputs = vec![base, lib, app];

        let plan = plan_bumps(
            &inputs,
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::SemverCascade,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        )
        .expect("plan");

        let by_module = |k: &ModuleKey| plan.entries.iter().find(|e| &e.module == k);
        assert_eq!(
            by_module(&base_key).expect("base").planned_version,
            Some(Version::new(0, 1, 1))
        );
        let lib_entry = by_module(&lib_key).expect("lib");
        assert_eq!(lib_entry.planned_version, None);
        assert!(!lib_entry.dep_floor_updates.is_empty());
        assert!(by_module(&app_key).is_none());
    }

    #[test]
    fn a_bumping_dependency_chain_cascades_through_every_dependent() {
        let base = {
            let mut input = dep_input("base", DependentVersion::Bump);
            input.changed = true;
            input
        };
        let lib = dep_input("lib", DependentVersion::Bump);
        let app = dep_input("app", DependentVersion::Bump);
        let (base_key, lib_key, app_key) = (key("base"), key("lib"), key("app"));
        let edges = vec![
            Edge::new(app_key.clone(), lib_key.clone(), DepKind::Normal),
            Edge::new(lib_key.clone(), base_key.clone(), DepKind::Normal),
        ];
        let graph =
            Graph::build(vec![module("base"), module("lib"), module("app")], edges).expect("graph");
        let overrides = BumpOverrides::new();
        let inputs = vec![base, lib, app];

        let plan = plan_bumps(
            &inputs,
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::SemverCascade,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        )
        .expect("plan");

        let by_module =
            |k: &ModuleKey| plan.entries.iter().find(|e| &e.module == k).expect("entry");
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
        assert!(!app_entry.dep_floor_updates.is_empty());
        assert_eq!(app_entry.reason, BumpReason::DependencyCascade);
    }

    #[test]
    fn incremental_decisions_match_the_batch_plan_in_dependency_first_order() {
        let base = {
            let mut input = dep_input("base", DependentVersion::Bump);
            input.changed = true;
            input
        };
        let lib = dep_input("lib", DependentVersion::Bump);
        let app = dep_input("app", DependentVersion::Bump);
        let (base_key, lib_key, app_key) = (key("base"), key("lib"), key("app"));
        let edges = vec![
            Edge::new(app_key.clone(), lib_key.clone(), DepKind::Normal),
            Edge::new(lib_key.clone(), base_key.clone(), DepKind::Normal),
        ];
        let graph =
            Graph::build(vec![module("app"), module("lib"), module("base")], edges).expect("graph");
        let overrides = BumpOverrides::new();
        let inputs = vec![app, base, lib];
        let config = BumpConfig {
            graph: &graph,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        };
        let expected = plan_bumps(&inputs, &config).expect("batch plan");

        let mut planner =
            BumpPlanner::new(inputs.iter().map(|input| input.module.clone()), &config)
                .expect("planner");
        assert_eq!(
            planner.modules(),
            &[base_key.clone(), lib_key.clone(), app_key.clone()]
        );
        let mut by_key = inputs
            .into_iter()
            .map(|input| (input.module.clone(), input))
            .collect::<BTreeMap<_, _>>();
        let mut resolved = Vec::new();
        for module in planner.modules().to_vec() {
            let input = by_key.remove(&module).expect("ordered input");
            let decision = planner.decide(input).expect("decision");
            resolved.push(decision.module.clone());
        }
        let actual = planner.finish().expect("incremental plan");

        assert_eq!(resolved, vec![base_key, lib_key, app_key]);
        assert_eq!(
            actual
                .entries
                .iter()
                .map(|entry| (
                    &entry.module,
                    &entry.planned_version,
                    &entry.dep_floor_updates
                ))
                .collect::<Vec<_>>(),
            expected
                .entries
                .iter()
                .map(|entry| (
                    &entry.module,
                    &entry.planned_version,
                    &entry.dep_floor_updates
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn incremental_planner_rejects_an_override_for_an_inactive_module() {
        let input = dep_input("core", DependentVersion::Bump);
        let graph = Graph::build(vec![module("core")], Vec::new()).expect("graph");
        let overrides = BumpOverrides::new()
            .with_module_level(key("core").module, BumpLevel::Major)
            .expect("override");
        let config = BumpConfig {
            graph: &graph,
            branches: &no_branches(),
            policy: BumpPolicy::SemverCascade,
            overrides: &overrides,
            intent: CutIntent::Verify,
        };
        let mut planner =
            BumpPlanner::new([input.module.clone()], &config).expect("planner accepts known module");
        planner.decide(input).expect("inactive decision");

        let error = planner
            .finish()
            .expect_err("an unchanged override must not silently no-op");
        assert!(error.to_string().contains("not in the release scope"), "{error}");
    }

    #[test]
    fn manifest_policy_cuts_the_declared_version_when_ahead_of_the_baseline() {
        let input = seed("core", "0.2.0", Some("0.1.0"), config());

        let plan = plan_seed(&[input], BumpPolicy::Manifest, CutIntent::Verify);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 2, 0)));
        assert_eq!(plan.entries[0].reason, BumpReason::Manifest);
    }

    #[test]
    fn manifest_policy_fails_closed_on_a_mutating_run_when_not_ahead_of_the_baseline() {
        let input = seed("core", "0.1.0", Some("0.1.0"), config());
        let modules = vec![module("core")];
        let graph = Graph::build(modules, Vec::new()).expect("graph");
        let overrides = BumpOverrides::new();

        let result = plan_bumps(
            &[input],
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::Manifest,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn manifest_preview_is_a_no_op_when_not_ahead_of_the_baseline() {
        let input = seed("core", "0.1.0", Some("0.1.0"), config());

        let plan = plan_seed(&[input], BumpPolicy::Manifest, CutIntent::Preview);

        assert!(plan.entries.is_empty());
    }

    #[test]
    fn semver_cascade_cuts_an_already_resolved_manifest_without_double_bumping() {
        // The manifest sits at 0.1.1 (a merged bump) over a 0.1.0 baseline tag:
        // anchoring on the baseline computes 0.1.1 and max(current, target) cuts
        // the already-resolved manifest version rather than 0.1.2.
        let input = seed("core", "0.1.1", Some("0.1.0"), config());

        let plan = plan_seed(&[input], BumpPolicy::SemverCascade, CutIntent::Verify);

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
    }

    #[test]
    fn pre_conflicts_with_the_manifest_policy() {
        let input = seed("core", "0.2.0", Some("0.1.0"), config());
        let modules = vec![module("core")];
        let graph = Graph::build(modules, Vec::new()).expect("graph");
        let overrides = BumpOverrides::new().with_prerelease("rc");

        let result = plan_bumps(
            &[input],
            &BumpConfig {
                graph: &graph,
                branches: &no_branches(),
                policy: BumpPolicy::Manifest,
                overrides: &overrides,
                intent: CutIntent::Verify,
            },
        );

        assert!(result.is_err());
    }
}
