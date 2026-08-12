use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_model::{Module, ModuleKey};
use toven_ports::{VcsReader, VcsWriter};

use crate::hosting::publish;
use crate::{ReleasePlan, ReleaseStats};

use super::guards::{
    forward_recovery_error, guard_clean_tree, guard_release_branch, restore_or_precommit_error,
};
use super::options::{ReleaseApplyOptions, reconcile_repo_settings};
use super::preflight::{
    TagPreflight, planned_tag_annotations, preflight_tag_signers, preflight_tags, preflight_targets,
};
use super::staging::{
    commit_message, package_publishable, prepare, publish_items, stage_and_commit,
};
use super::tagging::{push_refspecs, tag_releases};

/// Execute a [`ReleasePlan`] against the ecosystem release targets and the VCS.
///
/// `modules` must contain every module referenced by the plan; `targets` must
/// hold a release target for every ecosystem in the plan.
///
/// # Errors
/// Returns a typed error when the clean-tree guardrail trips, a module/target
/// is missing, a pre-commit mutation/package/commit fails (after restoring the
/// working tree), a VCS tag/push fails, or the publish loop exhausts its retry
/// budget.
pub fn release_apply(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &crate::ReleaseTargets,
    reader: &dyn VcsReader,
    writer: &dyn VcsWriter,
    options: &ReleaseApplyOptions,
) -> AppResult<ReleaseStats> {
    let mut stats = ReleaseStats::new(plan.entries.len());
    if plan.is_empty() {
        return Ok(stats);
    }

    let settings = reconcile_repo_settings(&plan.entries)?;
    // The branch and clean-tree guardrails run before any mutation.
    guard_release_branch(reader, settings.branches())?;
    guard_clean_tree(reader)?;

    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();

    // A maintainer-owned release runs against a tag/Release a human already
    // created: the Tag phase is an input, not a mutation. Verify the tags exist
    // for the planned version and publish against them — no manifest mutation,
    // no release commit, and nothing tagged or pushed. The hosted Release is
    // completed by the caller's create-or-verify host phase.
    if settings.entrypoint().is_maintainer_owned() {
        return maintainer_apply(plan, &module_by_ref, targets, reader, options, stats);
    }

    // Resolve all pre-commit errors before mutating any manifest.
    preflight_targets(plan, &module_by_ref, targets)?;
    let message = commit_message(plan, &module_by_ref, settings.commit_message())?;

    // If every planned tag already exists, the git mutation phase already ran
    // and pushed on a prior attempt: resume by skipping manifest mutation,
    // commit, tag, and push, and let the idempotent publish and hosted-release
    // phases finish. A partial tag overlap has already failed closed above.
    let tag_preflight = preflight_tags(plan, &module_by_ref, reader)?;
    if matches!(tag_preflight, TagPreflight::Resume) {
        return resume_apply(plan, &module_by_ref, targets, options, stats);
    }
    preflight_tag_signers(plan, writer)?;

    // Pre-commit phase (undoable): apply mutations, capture exactly the paths
    // they rewrote, then package every module that will be published.
    let (changed_paths, artifacts) = match prepare(plan, &module_by_ref, targets, &mut stats) {
        Ok(prepared) => prepared,
        Err(error) => return Err(restore_or_precommit_error(writer, "prepare", error)),
    };

    // Commit boundary. A release that rewrote manifests stages exactly those
    // paths and creates the release commit. A mutation-free release — a Go
    // tag-only cut, since Go carries no version in `go.mod` — rewrites nothing,
    // so it tags the existing `HEAD` instead of fabricating an empty release
    // commit. If staging or the commit fails, no history was created yet, so the
    // pre-commit working-tree mutations are still undoable.
    let created_commit = !changed_paths.is_empty();
    let commit = if created_commit {
        match stage_and_commit(writer, &changed_paths, &message) {
            Ok(commit) => commit,
            Err(error) => return Err(restore_or_precommit_error(writer, "commit", error)),
        }
    } else {
        reader.rev_parse("HEAD")?
    };

    // Post-commit phase (no rollback): tag, optionally push, publish. A failure
    // here cannot undo the release refs — it surfaces with forward-only recovery
    // guidance instead of pretending the run was atomic.
    let committed = || {
        if created_commit {
            format!("release commit {} was created", commit.as_str())
        } else {
            format!(
                "release tags were applied to existing commit {}",
                commit.as_str()
            )
        }
    };
    tag_releases(plan, &module_by_ref, writer, &commit, &mut stats)
        .map_err(|error| forward_recovery_error(&committed(), "tagging", error))?;
    if settings.pushes(options) {
        // Every push-phase step — resolving the branch (only when the branch
        // itself is pushed, so a tags-only push never needs one), computing
        // refspecs, and the push itself — runs after the commit and tags
        // exist, so any failure carries forward-only recovery guidance rather
        // than surfacing raw.
        let push = || -> AppResult<()> {
            let branch = settings
                .pushes_branch()
                .then(|| reader.current_branch())
                .transpose()?;
            let refspecs = push_refspecs(plan, branch.as_deref())?;
            if refspecs.is_empty() {
                return Ok(());
            }
            writer.push(settings.remote(), &refspecs)
        };
        push().map_err(|error| forward_recovery_error(&committed(), "push", error))?;
    }

    if options.publish {
        let items = publish_items(plan, &module_by_ref, targets, &artifacts)?;
        publish::run(&items, options.retry_budget, &mut stats)
            .map_err(|error| forward_recovery_error(&committed(), "publication", error))?;
    }

    Ok(stats)
}

/// Complete an already-tagged release without re-running the git mutation
/// phase.
///
/// Every planned tag already exists on the remote, so manifest mutation,
/// commit, tag, and push are skipped — the release commit and its immutable
/// tags were created and pushed on a prior attempt. Only the idempotent publish
/// loop runs: the manifest already carries the released version, so any version
/// the registry still lacks is packaged (without mutation) and published exactly
/// as a fresh run would, while an already-published version is not
/// `publish_needed` and is skipped, making a fully-published resume a clean
/// no-op. The hosted-release phase runs afterward in the caller, creating the
/// one Release a prior attempt left missing.
fn resume_apply(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    options: &ReleaseApplyOptions,
    mut stats: ReleaseStats,
) -> AppResult<ReleaseStats> {
    stats.resumed = true;
    if options.publish {
        // Package (no mutation) any version the registry still lacks so a
        // publish interrupted after tag/push can complete; a fully-published
        // resume packages nothing.
        let artifacts = package_publishable(plan, module_by_ref, targets, &mut stats)?;
        let items = publish_items(plan, module_by_ref, targets, &artifacts)?;
        publish::run(&items, options.retry_budget, &mut stats).map_err(|error| {
            forward_recovery_error(
                "the release commit, tags, and push already completed",
                "publication",
                error,
            )
        })?;
    }
    Ok(stats)
}

/// Apply a maintainer-owned release against an existing, human-created
/// tag/Release.
///
/// In this entrypoint the [`Tag`](toven_model::ReleasePhase::Tag) phase is an
/// **input**, not a mutation: a maintainer already created the release tag (and
/// the hosted Release) in the forge — the `release: published` flow — so Toven
/// verifies every planned tag exists for the planned version, failing closed on
/// absence or divergence, and then runs only the publish phase. No manifest is
/// mutated, no release commit is created, and nothing is tagged or pushed: the
/// version/CHANGELOG decision already merged through the `bump` phase, and the
/// hosted Release is completed by the caller's create-or-verify host phase.
/// Toven never creates or moves a maintainer-owned tag.
fn maintainer_apply(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    reader: &dyn VcsReader,
    options: &ReleaseApplyOptions,
    mut stats: ReleaseStats,
) -> AppResult<ReleaseStats> {
    preflight_targets(plan, module_by_ref, targets)?;
    verify_maintainer_tags(plan, module_by_ref, reader)?;
    if options.publish {
        // The manifest already carries the released version (the maintainer's
        // version/CHANGELOG PR merged), so packaging mutates nothing; publish
        // exactly the versions the registry still lacks against the existing
        // tags.
        let artifacts = package_publishable(plan, module_by_ref, targets, &mut stats)?;
        let items = publish_items(plan, module_by_ref, targets, &artifacts)?;
        publish::run(&items, options.retry_budget, &mut stats).map_err(|error| {
            forward_recovery_error(
                "the maintainer-created tags and hosted Release already exist",
                "publication",
                error,
            )
        })?;
    }
    Ok(stats)
}

/// Verify every planned release tag already exists for a maintainer-owned
/// release and points at the checked-out `HEAD`, failing closed otherwise.
///
/// The planned tag name encodes the planned version (it is rendered from the
/// module's tag scheme over the planned version), so a tag with that exact name
/// existing on the remote is the tag the maintainer created for this version. A
/// missing tag means either the maintainer has not cut the Release yet or the
/// manifest version and the created tag diverge.
///
/// Existence alone is not enough: a maintainer-owned run packages and publishes
/// artifacts from the checked-out `HEAD`, so a tag that points at a *different*
/// commit than `HEAD` (e.g. CI checks out a branch tip while the maintainer tag
/// references an earlier commit) would attach artifacts built from a commit the
/// tag does not name — a divergent Release. Every required tag must therefore
/// resolve to the `HEAD` commit; a diverging tag fails closed just like an
/// absent one. In neither case does Toven create or move the tag itself.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn verify_maintainer_tags(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    reader: &dyn VcsReader,
) -> AppResult<()> {
    let head = reader.rev_parse("HEAD")?;
    let existing = reader.list_tags(None)?;
    let target_by_name: BTreeMap<&str, &toven_ports::Oid> = existing
        .iter()
        .map(|tag| (tag.name.as_str(), &tag.target))
        .collect();
    let planned = planned_tag_annotations(plan, module_by_ref)?;
    let mut missing: Vec<&str> = Vec::new();
    let mut diverging: Vec<String> = Vec::new();
    for name in planned.keys().map(String::as_str) {
        match target_by_name.get(name) {
            None => missing.push(name),
            Some(target) if **target != head => {
                diverging.push(format!("{name} -> {}", target.as_str()));
            }
            Some(_) => {}
        }
    }
    if !missing.is_empty() {
        return Err(AppError::invalid_input(
            "release.entrypoint",
            format!(
                "maintainer-owned release requires the release tag(s) [{}] to already exist for \
                 the planned version, but they are absent; a maintainer must create the tag and \
                 hosted Release in the forge before Toven publishes against them — Toven never \
                 creates or moves a maintainer-owned tag",
                missing.join(", ")
            ),
        ));
    }
    if !diverging.is_empty() {
        return Err(AppError::invalid_input(
            "release.entrypoint",
            format!(
                "maintainer-owned release requires every release tag to point at the checked-out \
                 HEAD ({}), but [{}] reference other commits; Toven packages and publishes from \
                 HEAD, so it fails closed rather than attaching artifacts to a divergent tag — \
                 check out the maintainer's tagged commit before publishing",
                head.as_str(),
                diverging.join(", ")
            ),
        ));
    }
    Ok(())
}
