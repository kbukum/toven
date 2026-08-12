use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_core::config::Document;
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanContext, PlanRequest, prepare_front};
use toven_model::{MemberId, ModuleKey};
use toven_ports::{Provider, Reporter};
use toven_version::changelog;

use super::changelog_required::validate_required_changelogs;
use super::targets::{release_targets, resolve_release_settings};
use super::validation::reconcile_policy;
use crate::versioning::bump;
use crate::versioning::change;
use crate::{BumpOverrides, ReleaseBaseline, ReleasePlan, ResolvedReleaseSettings};

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
