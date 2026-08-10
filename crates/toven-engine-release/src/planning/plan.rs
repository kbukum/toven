//! Release PLAN tail orchestration.

use std::collections::BTreeMap;
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::read_string_bounded;
use toven_model::{EcosystemId, MemberId, ModuleKey};
use toven_ports::{Provider, PublicationPolicy, Reporter};

use toven_engine_core::config::Document;
use toven_engine_core::federation::baseline::MemberVcsReaders;
use toven_engine_core::federation::resolve::PathDriverLocator;
use toven_engine_core::plan::{PlanContext, PlanRequest, prepare_front};

use crate::versioning::bump;
use crate::versioning::change;
use crate::versioning::changelog;
use crate::{BumpOverrides, BumpPolicy, ReleaseBaseline, ReleasePlan, ResolvedReleaseSettings};

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
    plan_with_context(
        &context,
        request,
        readers,
        overrides,
        &targets,
        bump::CutIntent::Preview,
    )
}

/// Build a [`ReleasePlan`] from an already-prepared [`PlanContext`] and its
/// resolved release `targets`.
///
/// Shared by [`release_plan`] and the combined
/// [`release_run`](super::release_run) facade so the PLAN cut is computed by
/// exactly one path. `intent` selects whether a not-ahead `manifest` version is
/// reported as nothing-to-release (preview) or fails closed (mutating run).
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
    targets: &crate::ReleaseTargets,
    intent: bump::CutIntent,
) -> AppResult<ReleasePlan> {
    let settings = resolve_release_settings(context, targets)?;
    let changes = change::detect(
        context,
        overrides.base(),
        readers,
        targets,
        &settings,
        intent,
    )?;
    validate_required_changelogs(request.project_root.as_path(), &changes, &settings)?;
    let branches = current_branches(readers);
    plan_with_changes(
        context, request, &changes, &branches, overrides, targets, &settings, intent,
    )
}

/// Resolve each member's checked-out branch, best-effort.
///
/// A branch is recorded per member so a configured branch→prerelease-channel
/// mapping can select the channel from the checked-out branch. A member on a
/// detached HEAD (a common CI state) contributes no entry and simply resolves
/// to a stable release; branch resolution never fails a plan, because most
/// releases configure no branch→channel mapping at all.
fn current_branches(readers: &MemberVcsReaders<'_>) -> BTreeMap<Option<MemberId>, String> {
    readers
        .entries()
        .iter()
        .filter_map(|entry| {
            entry
                .reader()
                .current_branch()
                .ok()
                .map(|branch| (entry.member().cloned(), branch))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn plan_with_changes(
    context: &PlanContext,
    _request: &PlanRequest,
    changes: &change::ReleaseChanges,
    branches: &BTreeMap<Option<MemberId>, String>,
    overrides: &BumpOverrides,
    targets: &crate::ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    intent: bump::CutIntent,
) -> AppResult<ReleasePlan> {
    let policy = reconcile_policy(settings)?;
    let changelogs = context
        .federation
        .modules
        .iter()
        .map(|module| {
            let commits = changes
                .commits
                .get(&module.key())
                .cloned()
                .unwrap_or_default();
            let initial = changes
                .baselines
                .get(&module.key())
                .is_some_and(ReleaseBaseline::is_initial);
            (module.key(), changelog::entry(module, &commits, initial))
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
        branches,
        policy,
        overrides,
        intent,
    })?;

    validate_umbrella_tag_cut(context, settings, &entries)?;

    Ok(ReleasePlan::new(policy, entries))
}

/// Fail closed when the umbrella tag mode would release a train member with no
/// tag at all.
///
/// In [`TagMode::Umbrella`] a member's only tag is the shared umbrella tag, cut
/// solely by the umbrella module's own entry, and no per-module tags are
/// created. If the train releases other members but the umbrella module is not
/// itself bumped, it has no entry, so the umbrella tag is never cut and those
/// members would commit and publish entirely untagged. Refuse at plan time,
/// before any mutation, rather than relying on the umbrella module also
/// receiving a version bump. ([`TagMode::Both`] still cuts each changed
/// member's per-module tag, so it stays anchored even when the umbrella module
/// is unbumped.)
fn validate_umbrella_tag_cut(
    context: &PlanContext,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    entries: &[crate::ReleaseEntry],
) -> AppResult<()> {
    let released_members: std::collections::BTreeSet<Option<MemberId>> = entries
        .iter()
        .filter(|entry| entry.planned_version.is_some())
        .map(|entry| entry.module.member.clone())
        .collect();
    for module in &context.federation.modules {
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        if !resolved.umbrella {
            continue;
        }
        // Only pure `Umbrella` mode leaves a released member untagged; a mode
        // that also cuts per-module tags keeps changed members anchored.
        let umbrella_only = resolved
            .tag_mode
            .is_some_and(|mode| mode.creates_umbrella_tag() && !mode.creates_per_module_tags());
        if !umbrella_only || !released_members.contains(&module.member) {
            continue;
        }
        let umbrella_released = entries
            .iter()
            .any(|entry| entry.module == module.key() && entry.planned_version.is_some());
        if !umbrella_released {
            return Err(AppError::invalid_input(
                "release.umbrella",
                format!(
                    "tag mode 'umbrella' cuts only the member's umbrella tag from the umbrella \
                     module '{}', but the release bumps other members of its train without \
                     bumping the umbrella module, so the umbrella tag would never be cut and those \
                     members would publish untagged; ensure the umbrella module is released (for \
                     example by depending on its train members) or choose a per-module or both \
                     tag mode",
                    module.key()
                ),
            ));
        }
    }
    Ok(())
}

/// Resolve the release targets declared by each configured ecosystem adapter.
///
/// # Errors
/// Propagates a release target's construction failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn release_targets(context: &PlanContext) -> AppResult<crate::ReleaseTargets> {
    let mut targets = crate::ReleaseTargets::new();
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
    targets: &crate::ReleaseTargets,
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
            .map(toven_engine_core::federation::compose::ComposedMember::document)
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
        let resolved_settings = ResolvedReleaseSettings::resolve(&ecosystem, over)?;
        validate_ecosystem_publication(module, &resolved_settings)?;
        validate_visibility_compat(module, &resolved_settings)?;
        validate_phase_backing_supported(module, &resolved_settings)?;
        resolved.insert(module.key(), resolved_settings);
    }
    apply_adapter_release_defaults(context, targets, &mut resolved);
    validate_tag_mode_and_baseline(context, &resolved)?;
    Ok(resolved)
}

/// Fold each releaseable module's ecosystem adapter default tag mode and
/// baseline source into its [`ResolvedReleaseSettings`], completing the
/// documented precedence `[modules.<name>.release]` > `[ecosystems.<id>].release`
/// > adapter default. An explicit config value is left untouched.
///
/// An adapter's umbrella-anchored default (the registry-backed Rust model's
/// `registry+umbrella` baseline / `both` tag mode) is degraded to its per-module
/// counterpart for a train that does not declare exactly one umbrella module, so
/// an adapter default never forces an umbrella layout a train cannot satisfy —
/// mirroring change detection, which resolves the umbrella anchor per train
/// (member + ecosystem). A per-module ecosystem (Go) declares an own-tag /
/// per-module default that is unaffected by umbrella presence.
fn apply_adapter_release_defaults(
    context: &PlanContext,
    targets: &crate::ReleaseTargets,
    resolved: &mut BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) {
    let mut umbrella_count: BTreeMap<(Option<MemberId>, EcosystemId), usize> = BTreeMap::new();
    for module in &context.federation.modules {
        if resolved
            .get(&module.key())
            .is_some_and(|settings| settings.umbrella)
        {
            *umbrella_count
                .entry((module.member.clone(), module.id.ecosystem.clone()))
                .or_default() += 1;
        }
    }
    for module in &context.federation.modules {
        let train = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&train) else {
            continue;
        };
        let defaults = target.release_defaults();
        let train_has_single_umbrella =
            umbrella_count.get(&train).copied().unwrap_or_default() == 1;
        if let Some(settings) = resolved.get_mut(&module.key()) {
            settings.apply_adapter_defaults(defaults, train_has_single_umbrella);
        }
    }
}

/// The human-facing selector name when a module's tag mode or baseline requires
/// the member's umbrella tag, or `None` when neither does.
///
/// The tag mode takes precedence in the diagnostic (it is the more direct cause
/// of an umbrella-tag dependency); the baseline is reported only when the tag
/// mode does not itself require an umbrella.
fn umbrella_selector(
    tag_mode: Option<toven_ports::TagMode>,
    baseline: Option<toven_ports::BaselineSourceConfig>,
) -> Option<&'static str> {
    if tag_mode.is_some_and(toven_ports::TagMode::requires_umbrella) {
        return tag_mode.map(toven_ports::TagMode::as_str);
    }
    if baseline.is_some_and(toven_ports::BaselineSourceConfig::requires_umbrella) {
        return baseline.map(toven_ports::BaselineSourceConfig::as_str);
    }
    None
}

/// Fail closed when a `selector` that anchors on the train's umbrella tag finds
/// zero or more than one umbrella module in that train (member + ecosystem).
fn check_umbrella_count(module_id: &str, selector: &str, count: usize) -> AppResult<()> {
    if count == 0 {
        return Err(AppError::invalid_input(
            "release.umbrella",
            format!(
                "module '{module_id}' selects '{selector}', which anchors on the train's umbrella \
                 tag, but the train declares no umbrella module; mark one module `umbrella = \
                 true` or choose a per-module tag mode/baseline"
            ),
        ));
    }
    if count > 1 {
        return Err(AppError::invalid_input(
            "release.umbrella",
            format!(
                "module '{module_id}' selects '{selector}', which anchors on the train's umbrella \
                 tag, but the train declares {count} umbrella modules; a train has a single \
                 umbrella representative"
            ),
        ));
    }
    Ok(())
}

/// Fail closed when a module's resolved tag mode or baseline source references
/// an umbrella tag its train does not (uniquely) provide.
///
/// The umbrella tag is created by the train's single umbrella module, so a tag
/// mode that creates it ([`TagMode::Umbrella`](toven_ports::TagMode::Umbrella) /
/// [`TagMode::Both`](toven_ports::TagMode::Both)) and a baseline that anchors on
/// it (`umbrella-tag` / `registry+umbrella`) both require the train to declare
/// exactly one umbrella module. Zero umbrella modules leaves the umbrella tag
/// undefined; more than one makes it ambiguous. Both surface here at plan time —
/// before any mutation — rather than mid-apply.
///
/// Umbrella presence is counted per train (member + ecosystem), matching change
/// detection ([`train_umbrella_scheme`](crate::versioning)) and adapter-default
/// degradation ([`apply_adapter_release_defaults`]): a per-ecosystem tag scheme
/// makes the umbrella tag inherently per-ecosystem, so a polyglot member may
/// anchor one umbrella per ecosystem without conflict.
fn validate_tag_mode_and_baseline(
    context: &PlanContext,
    resolved: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<()> {
    let mut umbrella_count: BTreeMap<(Option<MemberId>, EcosystemId), usize> = BTreeMap::new();
    for module in &context.federation.modules {
        if resolved
            .get(&module.key())
            .is_some_and(|settings| settings.umbrella)
        {
            *umbrella_count
                .entry((module.member.clone(), module.id.ecosystem.clone()))
                .or_default() += 1;
        }
    }
    for module in &context.federation.modules {
        let Some(settings) = resolved.get(&module.key()) else {
            continue;
        };
        let Some(selector) = umbrella_selector(settings.tag_mode, settings.baseline) else {
            continue;
        };
        let count = umbrella_count
            .get(&(module.member.clone(), module.id.ecosystem.clone()))
            .copied()
            .unwrap_or_default();
        check_umbrella_count(&module.id.to_string(), selector, count)?;
    }
    Ok(())
}

/// Fail closed when a module requests a non-public [`Visibility`] against a
/// registry that can only publish public versions (crates.io today), so the
/// mismatch surfaces at plan time — before any tag, push, or publish — rather
/// than mid-mutation. The consuming registry adapter enforces the same rule as
/// a last line of defense; this keeps the failure fast and actionable.
fn validate_visibility_compat(
    module: &toven_model::Module,
    resolved: &ResolvedReleaseSettings,
) -> AppResult<()> {
    if resolved.visibility.is_public() {
        return Ok(());
    }
    if let PublicationPolicy::Registry { registry } = &resolved.publication
        && is_public_only_registry(registry)
    {
        return Err(AppError::invalid_input(
            "release.visibility",
            format!(
                "module '{}' requests visibility = {} but publishes to the public-only registry \
                 '{registry}'; publish to a registry that supports that exposure or set \
                 visibility = public",
                module.key(),
                resolved.visibility.as_str(),
            ),
        ));
    }
    Ok(())
}

/// Fail closed when a module delegates a phase Toven cannot dispatch through the
/// shared [`ToolRunner`](toven_ports::ToolRunner) seam, so a `backing =
/// "delegated"` entry never silently degrades to the native path.
///
/// Two rejections, both surfaced at plan time — before any mutation — naming the
/// phase and tool:
///
/// * a **non-delegable** phase ([`ReleasePhase::is_delegable`] is false —
///   selection, versioning, tag creation, registry publication, the hosted
///   Release) can never be delegated, because Toven owns the flow; and
/// * a delegable phase whose delegated execution is **not yet wired** at its
///   call site (`image`, `provenance`) is rejected until dispatch lands, rather
///   than accepted and run natively.
///
/// The dispatch-wired delegable phases (`package`, `sign`) are accepted; the
/// engine runs their delegated backing through the runner at the phase call
/// site.
fn validate_phase_backing_supported(
    module: &toven_model::Module,
    resolved: &ResolvedReleaseSettings,
) -> AppResult<()> {
    for phase in toven_model::ReleasePhase::ALL {
        let backing = resolved.phase_backing(*phase)?;
        let Some(tool) = backing.tool() else {
            continue;
        };
        if !phase.is_delegable() {
            return Err(AppError::invalid_input(
                format!("release.phases.{}", phase.as_str()),
                format!(
                    "module '{}' delegates the {} phase to '{tool}', but Toven owns the {} phase \
                     and never delegates it; only the package, sign, image, and provenance phases \
                     are delegable",
                    module.key(),
                    phase.as_str(),
                    phase.as_str(),
                ),
            ));
        }
        if !phase_delegation_dispatched(*phase) {
            return Err(AppError::invalid_input(
                format!("release.phases.{}", phase.as_str()),
                format!(
                    "module '{}' delegates the {} phase to '{tool}', but delegated {} execution is \
                     not yet wired; only the package and sign phases dispatch a delegated backing \
                     today, so leave the {} phase native (omit its entry) for now",
                    module.key(),
                    phase.as_str(),
                    phase.as_str(),
                    phase.as_str(),
                ),
            ));
        }
    }
    Ok(())
}

/// Whether a delegable phase's delegated backing is dispatched through the
/// runner at its engine call site today.
///
/// `package` and `sign` dispatch; `image` and `provenance` are delegable in
/// principle but their delegated execution is not yet wired, so
/// [`validate_phase_backing_supported`] rejects delegating them rather than
/// letting the config silently run native.
const fn phase_delegation_dispatched(phase: toven_model::ReleasePhase) -> bool {
    matches!(
        phase,
        toven_model::ReleasePhase::Package | toven_model::ReleasePhase::Sign
    )
}

/// Whether `registry` names a registry that only hosts public versions. This is
/// the engine's current registry-exposure knowledge: crates.io publishes every
/// version world-readable, so a non-public release cannot target it.
///
/// This is a known-public-only allow-list, so an *unrecognized* registry does
/// not trip this plan-time gate. That is safe because the registry adapter — not
/// this gate — is the authoritative closure: the only publishing adapter today
/// ([`toven_rust`]'s crates.io target) rejects every non-public exposure at the
/// toolchain boundary regardless of registry name. A future adapter for a
/// registry that *can* host private versions must itself honor or reject the
/// requested exposure; this list only makes the common crates.io mismatch fail
/// fast with an actionable message before any mutation.
fn is_public_only_registry(registry: &str) -> bool {
    matches!(registry, "crates-io" | "crates.io")
}

fn validate_ecosystem_publication(
    module: &toven_model::Module,
    resolved: &ResolvedReleaseSettings,
) -> AppResult<()> {
    if module.id.ecosystem.as_str() == "go"
        && matches!(resolved.publication, PublicationPolicy::Registry { .. })
    {
        return Err(AppError::invalid_input(
            "release.registry",
            format!(
                "Go module '{}' cannot declare a registry publication target; Go releases are tag-only",
                module.key()
            ),
        ));
    }
    if matches!(resolved.publication, PublicationPolicy::Excluded)
        && !resolved.host.assets.is_empty()
    {
        return Err(AppError::invalid_input(
            "release.exclude",
            format!(
                "excluded module '{}' cannot declare hosted release assets",
                module.key()
            ),
        ));
    }
    Ok(())
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

/// Fail closed when a directly changed release unit lacks its required,
/// file-backed changelog evidence.
///
/// The configured changelog is read from its project-relative path and must
/// carry a documented `## [Unreleased]` section (see
/// [`changelog::unreleased_documented`]). A missing, unreadable, or
/// undocumented changelog is a typed configuration failure surfaced before any
/// mutation. Modules selected only through a dependency cascade are not directly
/// changed and are exempt — their release reason is the cascade explanation
/// carried by their [`ChangelogEntry`](crate::ChangelogEntry).
fn validate_required_changelogs(
    project_root: &Path,
    changes: &change::ReleaseChanges,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<()> {
    /// Upper bound on a changelog read; a document larger than this is treated
    /// as malformed rather than loaded unbounded.
    const MAX_CHANGELOG_BYTES: u64 = 4 * 1024 * 1024;

    let mut documented: BTreeMap<String, bool> = BTreeMap::new();
    for module in &changes.changed {
        let Some(resolved) = settings.get(module) else {
            continue;
        };
        if !resolved.changelog.required {
            continue;
        }
        let relative = resolved.changelog.path.as_deref().unwrap_or("CHANGELOG.md");
        if let Some(has_entry) = documented.get(relative) {
            if !*has_entry {
                return Err(undocumented_changelog_error(module, relative));
            }
            continue;
        }
        let absolute = safe_join(project_root, relative).map_err(|error| {
            AppError::invalid_input(
                "release.changelog.path",
                format!("changelog path '{relative}' is not a safe project-relative path"),
            )
            .with_cause(error)
        })?;
        let text = read_string_bounded(&absolute, MAX_CHANGELOG_BYTES).map_err(|error| {
            AppError::invalid_input(
                "release.changelog.required",
                format!(
                    "required changelog '{relative}' for changed module '{module}' could not be \
                     read; create it and document the change before releasing"
                ),
            )
            .with_cause(error)
        })?;
        let has_entry = changelog::unreleased_documented(&text);
        documented.insert(relative.to_string(), has_entry);
        if !has_entry {
            return Err(undocumented_changelog_error(module, relative));
        }
    }
    Ok(())
}

/// The typed failure for a changed module whose required changelog has no
/// documented `[Unreleased]` entry.
fn undocumented_changelog_error(module: &ModuleKey, relative: &str) -> AppError {
    AppError::invalid_input(
        "release.changelog.required",
        format!(
            "changed module '{module}' requires a documented '[Unreleased]' entry in \
             '{relative}', but none was found; record the change before releasing"
        ),
    )
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
        CommonEcosystemConfig, DependentVersion, DiscoverResponse, Oid, PrereleaseConfig, Provider,
        PublicationPolicy, ReleaseConfig, TagRef, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::{
        BumpPolicy, ResolvedReleaseSettings, check_umbrella_count, reconcile_policy, release_plan,
        umbrella_selector, validate_phase_backing_supported, validate_required_changelogs,
    };
    use crate::{BumpOverrides, BumpReason, BumpSource, ReleasePlan};
    use toven_engine_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_engine_core::federation::baseline::MemberVcsReaders;
    use toven_engine_core::plan::{PlanRequest, Selection};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(eid("rust"), name).unwrap()
    }

    fn module(name: &str, root: &str) -> Module {
        Module::new(mref(name), RepoPath::new(root).unwrap())
    }

    #[test]
    fn a_native_phase_backing_passes_the_support_guard() {
        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert!(
            validate_phase_backing_supported(&module("core", "core"), &resolved).is_ok(),
            "an unconfigured (native) phase backing must be accepted"
        );
    }

    #[test]
    fn per_module_tag_mode_and_own_tag_baseline_need_no_umbrella() {
        use toven_ports::{BaselineSourceConfig, TagMode};
        assert!(
            umbrella_selector(Some(TagMode::PerModule), Some(BaselineSourceConfig::OwnTag))
                .is_none()
        );
        assert!(umbrella_selector(None, None).is_none());
        assert!(umbrella_selector(None, Some(BaselineSourceConfig::Registry)).is_none());
    }

    #[test]
    fn umbrella_tag_mode_and_umbrella_baseline_require_an_umbrella() {
        use toven_ports::{BaselineSourceConfig, TagMode};
        assert_eq!(
            umbrella_selector(Some(TagMode::Umbrella), None),
            Some("umbrella")
        );
        assert_eq!(umbrella_selector(Some(TagMode::Both), None), Some("both"));
        assert_eq!(
            umbrella_selector(None, Some(BaselineSourceConfig::UmbrellaTag)),
            Some("umbrella-tag")
        );
        assert_eq!(
            umbrella_selector(None, Some(BaselineSourceConfig::RegistryUmbrella)),
            Some("registry+umbrella")
        );
    }

    #[test]
    fn umbrella_selector_reports_the_tag_mode_before_the_baseline() {
        use toven_ports::{BaselineSourceConfig, TagMode};
        assert_eq!(
            umbrella_selector(
                Some(TagMode::Umbrella),
                Some(BaselineSourceConfig::UmbrellaTag)
            ),
            Some("umbrella")
        );
    }

    #[test]
    fn an_umbrella_selector_with_zero_or_many_umbrellas_fails_closed() {
        let zero = check_umbrella_count("rust:core", "umbrella", 0)
            .expect_err("zero umbrella modules must fail closed");
        assert!(zero.to_string().contains("no umbrella module"), "{zero}");

        let many = check_umbrella_count("rust:core", "umbrella", 2)
            .expect_err("multiple umbrella modules must fail closed");
        assert!(many.to_string().contains("single umbrella"), "{many}");

        assert!(check_umbrella_count("rust:core", "umbrella", 1).is_ok());
    }

    #[test]
    fn a_delegated_delegable_phase_is_accepted() {
        use toven_model::ReleasePhase;
        use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

        let mut phases = BTreeMap::new();
        phases.insert(
            ReleasePhase::Package,
            PhaseConfig {
                backing: PhaseBackingKind::Delegated,
                delegated: Some(DelegatedTool {
                    tool: "goreleaser".into(),
                    args: Some(vec!["release".into()]),
                    preview: vec!["release".into(), "--snapshot".into()],
                }),
            },
        );
        let ecosystem = ReleaseConfig {
            phases: Some(PhasesConfig(phases)),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

        assert!(
            validate_phase_backing_supported(&module("core", "core"), &resolved).is_ok(),
            "delegating the package phase (delegable) must be accepted"
        );
    }

    #[test]
    fn delegating_a_flow_ownership_phase_is_rejected() {
        use toven_model::ReleasePhase;
        use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

        // Publish is a flow-ownership phase: Toven owns registry publication and
        // never delegates it, so a delegated backing must fail closed at plan
        // time rather than degrade to native.
        let mut phases = BTreeMap::new();
        phases.insert(
            ReleasePhase::Publish,
            PhaseConfig {
                backing: PhaseBackingKind::Delegated,
                delegated: Some(DelegatedTool {
                    tool: "goreleaser".into(),
                    args: Some(vec!["release".into()]),
                    preview: vec!["release".into(), "--snapshot".into()],
                }),
            },
        );
        let ecosystem = ReleaseConfig {
            phases: Some(PhasesConfig(phases)),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

        let error = validate_phase_backing_supported(&module("core", "core"), &resolved)
            .expect_err("delegating a flow-ownership phase must fail closed");
        let message = error.to_string();
        assert!(message.contains("publish"), "{message}");
        assert!(message.contains("goreleaser"), "{message}");
        assert!(
            message.contains("never delegates"),
            "the error must explain Toven owns the phase: {message}"
        );
    }

    #[test]
    fn delegating_a_not_yet_wired_delegable_phase_is_rejected() {
        use toven_model::ReleasePhase;
        use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

        // Provenance is delegable in principle, but its delegated execution is
        // not yet wired at the call site — accepting it would silently run
        // native, so it must fail closed until dispatch lands.
        let mut phases = BTreeMap::new();
        phases.insert(
            ReleasePhase::Provenance,
            PhaseConfig {
                backing: PhaseBackingKind::Delegated,
                delegated: Some(DelegatedTool {
                    tool: "goreleaser".into(),
                    args: Some(vec!["release".into()]),
                    preview: vec!["release".into(), "--snapshot".into()],
                }),
            },
        );
        let ecosystem = ReleaseConfig {
            phases: Some(PhasesConfig(phases)),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

        let error = validate_phase_backing_supported(&module("core", "core"), &resolved)
            .expect_err("delegating a not-yet-wired phase must fail closed");
        let message = error.to_string();
        assert!(message.contains("provenance"), "{message}");
        assert!(
            message.contains("not yet wired"),
            "the error must explain dispatch is unimplemented: {message}"
        );
    }

    /// Release tags placing every named module at `0.1.0`, so change detection
    /// diffs against a real prior release instead of planning a first release.
    fn released_at_0_1_0(modules: &[&str]) -> Vec<TagRef> {
        modules
            .iter()
            .map(|name| TagRef::new(format!("rust/{name}@0.1.0"), Oid::new("cafe")))
            .collect()
    }

    fn module_for_ecosystem(ecosystem: &str, name: &str, root: &str) -> Module {
        Module::new(
            ModuleRef::new(eid(ecosystem), name).unwrap(),
            RepoPath::new(root).unwrap(),
        )
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
            hooks: std::collections::BTreeMap::new(),
        }
    }

    fn document_for_ecosystem(ecosystem: &str, release: &serde_json::Value) -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(
            eid(ecosystem),
            RawValue::from(json!({ "release": release.clone() })),
        );
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
            hooks: std::collections::BTreeMap::new(),
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
        let vcs = FakeVcsReader::new()
            .with_tags(released_at_0_1_0(&["core"]))
            .with_changed_since(vec![ChangeRecord::new(
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

    fn common_with_registry() -> CommonEcosystemConfig {
        CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        }
    }

    #[test]
    fn go_registry_publication_is_rejected_during_release_resolution() {
        let module = module_for_ecosystem("go", "app", ".");
        let mut response = DiscoverResponse::new(eid("go"));
        response.modules = vec![module];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("goproxy".into()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("go"))
            .with_response(response)
            .with_common(common)
            .with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(eid("go")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let request = PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let error = release_plan(
            &request,
            &document_for_ecosystem("go", &json!({ "registry": "goproxy" })),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
        )
        .expect_err("Go registry publication is invalid");

        assert!(error.to_string().contains("Go module"));
        assert!(error.to_string().contains("registry"));
    }

    /// Drive `release_plan` against one rust `core` module whose ecosystem
    /// release config is `release`, returning the resolution result.
    fn plan_with_release_config(
        release: ReleaseConfig,
    ) -> rskit_errors::AppResult<crate::ReleasePlan> {
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let common = CommonEcosystemConfig {
            release,
            ..CommonEcosystemConfig::default()
        };
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
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
        )
    }

    /// Drive `release_plan` against one rust `core` module whose ecosystem
    /// release config is `release`, returning the resolution error.
    fn plan_error_with_release_config(release: ReleaseConfig) -> rskit_errors::AppError {
        plan_with_release_config(release).expect_err("the configured setting must fail closed")
    }

    #[test]
    fn private_visibility_to_a_public_only_registry_is_rejected() {
        // crates.io publishes every version world-readable, so a private/internal
        // release cannot target it — fail closed at plan time, before any tag or
        // push, rather than silently publishing it publicly.
        let error = plan_error_with_release_config(ReleaseConfig {
            registry: Some("crates-io".into()),
            visibility: Some(toven_ports::Visibility::Private),
            ..ReleaseConfig::default()
        });

        let message = error.to_string();
        assert!(message.contains("release.visibility"), "{message}");
        assert!(message.contains("public-only"), "{message}");
    }

    #[test]
    fn private_visibility_on_a_tag_only_release_is_accepted() {
        // A private release that never publishes to a public registry (tag-only)
        // has no public-only mutation to conflict with, so it resolves cleanly.
        plan_with_release_config(ReleaseConfig {
            publish: Some(false),
            visibility: Some(toven_ports::Visibility::Private),
            ..ReleaseConfig::default()
        })
        .expect("a tag-only private release must be accepted");
    }

    #[test]
    fn enabled_artifact_signing_is_accepted_now_that_signing_is_executable() {
        // Signing is a real capability (`toven release sign` via cosign), so
        // `sign.enabled = true` must no longer be rejected during planning —
        // the plan resolves and the release proceeds to the executable signer.
        let plan = plan_with_release_config(ReleaseConfig {
            sign: Some(toven_ports::SignConfig {
                enabled: true,
                signer: None,
                ..toven_ports::SignConfig::default()
            }),
            ..ReleaseConfig::default()
        })
        .expect("sign.enabled must be accepted");
        assert!(
            !plan.entries.is_empty(),
            "the release plan resolves with signing enabled"
        );
    }

    #[test]
    fn branch_channel_mapping_cuts_a_prerelease_from_the_checked_out_branch() {
        // With a `next -> beta` mapping and the reader checked out on `next`,
        // a changed module cuts a `beta` prerelease with no `--pre` flag.
        let mut common = CommonEcosystemConfig::default();
        common.release.prerelease = Some(toven_ports::PrereleaseConfig {
            channels: vec!["beta".into()],
            branch_channels: std::collections::BTreeMap::from([("next".into(), "beta".into())]),
        });
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
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
        let vcs = FakeVcsReader::new()
            .with_current_branch("next")
            .with_tags(released_at_0_1_0(&["core"]))
            .with_changed_since(vec![ChangeRecord::new(
                "crates/core/src/lib.rs",
                ChangeStatus::Modified,
            )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
        )
        .unwrap();
        assert_eq!(
            plan.entries[0].planned_version,
            Some(Version::parse("0.1.1-beta.1").unwrap())
        );
        assert_eq!(plan.entries[0].prerelease_channel.as_deref(), Some("beta"));
    }

    #[test]
    fn branch_channel_mapping_is_stable_on_an_unmapped_branch() {
        // The same `next -> beta` mapping on the default `main` branch cuts a
        // stable release: only a mapped branch selects a prerelease channel.
        let mut common = CommonEcosystemConfig::default();
        common.release.prerelease = Some(toven_ports::PrereleaseConfig {
            channels: vec!["beta".into()],
            branch_channels: std::collections::BTreeMap::from([("next".into(), "beta".into())]),
        });
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
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
        let vcs = FakeVcsReader::new()
            .with_current_branch("main")
            .with_tags(released_at_0_1_0(&["core"]))
            .with_changed_since(vec![ChangeRecord::new(
                "crates/core/src/lib.rs",
                ChangeStatus::Modified,
            )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
        )
        .unwrap();
        assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
        assert_eq!(plan.entries[0].prerelease_channel, None);
    }

    #[test]
    fn token_env_on_a_registry_module_is_accepted_and_carried_to_the_plan_entry() {
        // `token_env` names the environment variable that holds the registry
        // token; the publishing adapter reads it at the toolchain boundary and
        // injects the credential, so a registry module configuring it plans
        // cleanly (no rejection) and the resolved name rides onto the entry for
        // publish-time injection.
        let mut common = CommonEcosystemConfig::default();
        common.release.registry = Some("crates-io".into());
        common.release.token_env = Some("CARGO_REGISTRY_TOKEN".into());
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
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
        let vcs = FakeVcsReader::new()
            .with_tags(released_at_0_1_0(&["core"]))
            .with_changed_since(vec![ChangeRecord::new(
                "crates/core/src/lib.rs",
                ChangeStatus::Modified,
            )]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();
        let plan = release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
        )
        .expect("a registry module with a configured token_env must plan cleanly");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.entries[0].token_env.as_deref(),
            Some("CARGO_REGISTRY_TOKEN"),
            "the resolved token_env must ride onto the plan entry for publish-time injection"
        );
    }

    #[test]
    fn token_env_on_a_tag_only_module_is_harmless_and_accepted() {
        // `token_env` only claims meaning for registry publication; a tag-only
        // module never publishes, so the inert field is not a safety risk.
        let core = module("core", "crates/core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                token_env: Some("CARGO_REGISTRY_TOKEN".into()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
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
        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
        )
        .expect("a tag-only module's inert token_env must not fail the plan");
    }

    #[test]
    fn required_changelog_rejects_a_missing_file() {
        let temp = rskit_fs::TempDir::new().expect("temp dir");
        let error =
            validate_required_changelogs(temp.path(), &changes_for("core"), &required_core())
                .expect_err("a missing required changelog must fail closed");

        assert!(error.to_string().contains("release.changelog.required"));
        assert!(error.to_string().contains("rust:core"));
    }

    #[test]
    fn required_changelog_rejects_an_empty_unreleased_section() {
        let temp = rskit_fs::TempDir::new().expect("temp dir");
        std::fs::write(
            temp.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Added\n\n## [1.0.0]\n\n- Shipped\n",
        )
        .expect("write changelog");

        let error =
            validate_required_changelogs(temp.path(), &changes_for("core"), &required_core())
                .expect_err("an undocumented Unreleased section must fail closed");

        assert!(error.to_string().contains("Unreleased"));
        assert!(error.to_string().contains("rust:core"));
    }

    #[test]
    fn required_changelog_accepts_a_documented_unreleased_section() {
        let temp = rskit_fs::TempDir::new().expect("temp dir");
        std::fs::write(
            temp.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Reworked the release planner\n",
        )
        .expect("write changelog");

        validate_required_changelogs(temp.path(), &changes_for("core"), &required_core())
            .expect("a documented Unreleased section satisfies the requirement");
    }

    #[test]
    fn changelog_not_required_skips_the_file_check() {
        // No changelog file exists, but the module does not require one.
        let temp = rskit_fs::TempDir::new().expect("temp dir");
        let key = ModuleKey::bare(mref("core"));
        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        let settings = BTreeMap::from([(key, resolved)]);

        validate_required_changelogs(temp.path(), &changes_for("core"), &settings)
            .expect("an unrequired changelog is not read");
    }

    fn changes_for(name: &str) -> crate::versioning::change::ReleaseChanges {
        let mut changed = std::collections::BTreeSet::new();
        changed.insert(ModuleKey::bare(mref(name)));
        crate::versioning::change::ReleaseChanges {
            changed,
            records: BTreeMap::new(),
            commits: BTreeMap::new(),
            baselines: BTreeMap::new(),
        }
    }

    fn required_core() -> BTreeMap<ModuleKey, ResolvedReleaseSettings> {
        let mut config = CommonEcosystemConfig::default();
        config.release.changelog = Some(ChangelogConfig {
            path: None,
            required: true,
            roll: false,
        });
        let resolved = ResolvedReleaseSettings::resolve(&config.release, None).unwrap();
        BTreeMap::from([(ModuleKey::bare(mref("core")), resolved)])
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
            .with_common(common_with_registry())
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
            .with_tags(released_at_0_1_0(&["core", "app"]))
            .with_changed_since(vec![ChangeRecord::new(
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
        assert_eq!(
            plan.entries[0].publication,
            PublicationPolicy::Registry {
                registry: "crates-io".into()
            }
        );
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
            .with_common(common_with_registry())
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
            .with_tags(released_at_0_1_0(&["core", "mid", "top"]))
            .with_changed_since(vec![ChangeRecord::new(
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
    fn release_plan_generates_grouped_attributed_changelog_from_commits() {
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
            .with_tags(released_at_0_1_0(&["core", "app"]))
            .with_changed_since(vec![
                ChangeRecord::new("crates/core/src/lib.rs", ChangeStatus::Modified),
                ChangeRecord::new("crates/app/src/lib.rs", ChangeStatus::Modified),
            ])
            .with_worktree_status(vec![ChangeRecord::new(
                "crates/app/src/main.rs",
                ChangeStatus::Modified,
            )])
            .with_commits_since(vec![
                toven_ports::CommitSummary::new("abc123def456", "feat(core): add widget")
                    .with_author("Ada", "42+ada@users.noreply.github.com"),
                toven_ports::CommitSummary::new("def456abc789", "fix: correct off-by-one")
                    .with_author("Bo", "bo@example.com"),
            ]);
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

        // The scripted commits are rendered as grouped, attributed Keep a
        // Changelog bullets — the GitHub noreply email becomes an `@handle`, the
        // non-GitHub author falls back to a name, and each commit lands under its
        // Conventional Commit group heading.
        let core_lines = by_module.get(&core.key()).unwrap();
        assert_eq!(
            core_lines,
            &vec![
                "### Added".to_string(),
                "- **core**: add widget — by @ada (abc123def456)".to_string(),
                String::new(),
                "### Fixed".to_string(),
                "- correct off-by-one — by Bo (def456abc789)".to_string(),
            ]
        );
        // The double returns the same commits for every module (path scoping is
        // the adapter's job, covered by the real-repo `commits_since` test), so
        // `app` renders the same grouped changelog.
        assert_eq!(by_module.get(&app.key()).unwrap(), core_lines);
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
        let vcs = FakeVcsReader::new()
            .with_tags(released_at_0_1_0(&["core"]))
            .with_changed_since(vec![ChangeRecord::new(
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
        let plan = plan_core(common_with_registry(), target, &BumpOverrides::new());
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
        let plan = plan_core(common_with_registry(), target, &overrides);
        assert!(!plan.entries[0].up_to_date);
        assert!(plan.entries[0].publish_needed);
    }

    #[test]
    fn default_publication_policy_is_tag_only() {
        let plan = plan_core(
            CommonEcosystemConfig::default(),
            FakeReleaseTarget::new(),
            &BumpOverrides::new(),
        );

        assert!(!plan.entries[0].publish_needed);
        assert_eq!(plan.entries[0].publication, PublicationPolicy::TagOnly);
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
        let vcs = FakeVcsReader::new()
            .with_tags(released_at_0_1_0(&["core", "app"]))
            .with_changed_since(vec![ChangeRecord::new(
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
        assert_eq!(app_entry.planned_tag, None);
        assert!(!app_entry.publish_needed);
        assert_eq!(
            app_entry.mutation.dep_floor_updates.get(&mref("core")),
            Some(&Version::new(0, 1, 1))
        );
    }

    #[test]
    fn planned_entries_carry_the_release_tag_name() {
        // The plan must explain the exact tag a mutating run would create.
        let plan = plan_core(
            common_with_level(BumpLevel::Minor),
            FakeReleaseTarget::new(),
            &BumpOverrides::new(),
        );

        let entry = plan.entries.first().expect("one planned entry");
        assert_eq!(entry.planned_version, Some(Version::new(0, 2, 0)));
        assert_eq!(entry.planned_tag.as_deref(), Some("rust/core@0.2.0"));
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
