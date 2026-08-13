use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::plan::PlanContext;
use toven_model::{EcosystemId, MemberId, ModuleKey};

use super::validation::{
    validate_ecosystem_publication, validate_phase_backing_supported,
    validate_tag_mode_and_baseline, validate_visibility_compat,
};
use crate::ResolvedReleaseSettings;

/// Resolve the release targets declared by each configured ecosystem adapter.
///
/// # Errors
/// Propagates a release target's construction failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn release_targets(
    context: &PlanContext,
    readers: &MemberVcsReaders<'_>,
) -> AppResult<crate::ReleaseTargets> {
    let mut targets = crate::ReleaseTargets::new();
    for (member, ecosystem, adapter) in context.adapters.iter() {
        let reader = readers
            .entries()
            .iter()
            .find(|entry| entry.member() == member)
            .map(toven_core::federation::baseline::MemberVcsReader::reader)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "configured adapter for ecosystem '{ecosystem}' has no VCS reader for its member"
                    ),
                )
            })?;
        if let Some(target) = adapter.release_target(reader)? {
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
            .map(toven_core::federation::compose::ComposedMember::document)
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
