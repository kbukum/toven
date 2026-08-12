use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_core::plan::PlanContext;
use toven_model::{EcosystemId, MemberId, ModuleKey};
use toven_ports::PublicationPolicy;

use crate::{BumpPolicy, ResolvedReleaseSettings};

/// The human-facing selector name when a module's tag mode or baseline requires
/// the member's umbrella tag, or `None` when neither does.
///
/// The tag mode takes precedence in the diagnostic (it is the more direct cause
/// of an umbrella-tag dependency); the baseline is reported only when the tag
/// mode does not itself require an umbrella.
pub(super) fn umbrella_selector(
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
pub(super) fn check_umbrella_count(module_id: &str, selector: &str, count: usize) -> AppResult<()> {
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
pub(super) fn validate_tag_mode_and_baseline(
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
pub(super) fn validate_visibility_compat(
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
pub(super) fn validate_phase_backing_supported(
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

pub(super) fn validate_ecosystem_publication(
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
pub(super) fn reconcile_policy(
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
