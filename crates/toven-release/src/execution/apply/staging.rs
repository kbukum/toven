use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{Module, ModuleKey, RepoPath};
use toven_ports::{Artifact, ReleaseAdapter, ReleaseCredentials, VcsWriter};

use crate::hosting::publish::PublishItem;
use crate::{ReleasePlan, ReleaseStats};

use super::tagging::{planned_tag_name, render_template};

/// Apply every mutation, capture the working-tree paths those mutations
/// rewrote, then package every module that will be published, returning the
/// changed paths and the artifacts keyed by module. Runs entirely before the
/// commit so the caller can restore the working tree on failure.
///
/// The changed-path snapshot is taken **after** applying mutations but
/// **before** packaging, so it reflects exactly the manifests the release
/// rewrote and never captures release artifacts a target may write into the
/// working tree. The clean-tree guard ran before any mutation, so any path here
/// is the release's own write. An empty snapshot means the release mutated no
/// manifest — a Go tag-only cut — and the caller tags `HEAD` rather than
/// creating an empty commit.
///
/// Packaging is scoped to `publish_needed` entries: a tag-only module (and a
/// registry module whose version is already published) produces no packaged
/// artifact, because none is consumed by the publish loop. This also keeps a
/// tag-only release from invoking ecosystem packaging that cannot succeed —
/// e.g. `cargo package` on an unpublished workspace crate whose intra-workspace
/// dependencies are not resolvable from the registry.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepare(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    stats: &mut ReleaseStats,
) -> AppResult<(
    Vec<RepoPath>,
    BTreeMap<ModuleKey, Artifact>,
    crate::execution::mutate::MutatedManifests,
)> {
    let mutated = crate::execution::mutate::mutate_manifests(plan, module_by_ref, targets, stats)?;
    let changed_paths = crate::execution::mutate::staged_paths(&mutated);
    let artifacts = package_publishable(plan, module_by_ref, targets, stats)?;
    Ok((changed_paths, artifacts, mutated))
}

/// Stage exactly the release-mutated paths and create the release commit.
///
/// `changed_paths` are the repo-relative manifests the release's mutations
/// reported rewriting, so committing them makes the commit carry the version
/// bump (a bare commit would otherwise write an empty tree and leave the bump
/// dangling in the working tree). Only called when that set is non-empty.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn stage_and_commit(
    writer: &dyn VcsWriter,
    changed_paths: &[RepoPath],
    message: &str,
) -> AppResult<toven_ports::Oid> {
    let staged = staged_refs(changed_paths)?;
    let staged_refs: Vec<&str> = staged.iter().map(String::as_str).collect();
    writer.commit(message, &staged_refs)
}

/// Stage exactly the release-mutated paths without creating a commit.
///
/// The PR-first `bump` path stages the version/changelog mutation
/// for a maintainer's pull request instead of cutting the release commit,
/// reusing the same repo-relative path set [`stage_and_commit`] would commit.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn stage_only(writer: &dyn VcsWriter, changed_paths: &[RepoPath]) -> AppResult<()> {
    let staged = staged_refs(changed_paths)?;
    let staged_refs: Vec<&str> = staged.iter().map(String::as_str).collect();
    writer.stage(&staged_refs)
}

/// Render the repo-relative mutated paths as staged string refs, failing closed
/// on a non-UTF-8 path.
fn staged_refs(changed_paths: &[RepoPath]) -> AppResult<Vec<String>> {
    changed_paths
        .iter()
        .map(|path| {
            path.as_path().to_str().map(str::to_owned).ok_or_else(|| {
                AppError::invalid_input(
                    "path",
                    format!("non-UTF-8 repo path '{}'", path.as_path().display()),
                )
            })
        })
        .collect()
}

/// Package every `publish_needed` entry without mutating any manifest.
///
/// The fresh path calls this after applying mutations; the resume path calls it
/// alone. On a resume the release commit, tags, and push already exist and the
/// manifest already carries the released version, so no mutation is needed —
/// only the artifact the idempotent publish loop consumes for a version the
/// registry still lacks. An already-published entry is not `publish_needed`, so
/// a fully-published resume packages nothing, matching the fresh path's skip.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn package_publishable(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleKey, Artifact>> {
    let mut artifacts = BTreeMap::new();
    for entry in &plan.entries {
        if !entry.publish_needed {
            continue;
        }
        let module = module_for(module_by_ref, &entry.module)?;
        let target = target_for(targets, module)?;
        artifacts.insert(entry.module.clone(), target.package(module)?);
        stats.packaged_artifacts += 1;
    }
    Ok(artifacts)
}

/// Resolve the ordered publish items, skipping entries that need no publish.
///
/// Each item carries the registry credential context resolved from *its* module
/// entry (the `token_env` variable name, never the secret); a module without a
/// configured `token_env` publishes with the toolchain's ambient credential.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn publish_items<'a>(
    plan: &'a ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &'a Module>,
    targets: &'a crate::ReleaseTargets,
    artifacts: &'a BTreeMap<ModuleKey, Artifact>,
) -> AppResult<Vec<PublishItem<'a>>> {
    let mut items = Vec::new();
    for entry in &plan.entries {
        if !entry.publish_needed {
            continue;
        }
        let module = module_for(module_by_ref, &entry.module)?;
        // A publish-needed entry is always packaged with a planned version in the
        // pre-commit phase; a missing one is an internal inconsistency, not a skip.
        let (Some(version), Some(artifact)) =
            (entry.planned_version.as_ref(), artifacts.get(&entry.module))
        else {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "module '{}' is marked publish-needed but has no planned version or artifact",
                    entry.module
                ),
            ));
        };
        items.push(PublishItem {
            module,
            target: target_for(targets, module)?,
            artifact,
            version,
            credentials: ReleaseCredentials::new(
                entry.token_env.clone(),
                entry.publication.registry().map(str::to_string),
            ),
            visibility: entry.visibility,
        });
    }
    Ok(items)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn module_for<'a>(
    module_by_ref: &BTreeMap<ModuleKey, &'a Module>,
    reference: &ModuleKey,
) -> AppResult<&'a Module> {
    module_by_ref.get(reference).copied().ok_or_else(|| {
        AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
    })
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn target_for<'a>(
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

/// Build the single release commit message from the released module versions.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn commit_message(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    template: Option<&str>,
) -> AppResult<String> {
    if let Some(template) = template {
        let mut messages = BTreeSet::new();
        for entry in &plan.entries {
            let Some(version) = &entry.planned_version else {
                continue;
            };
            let module = module_for(module_by_ref, &entry.module)?;
            messages.insert(render_template(
                template,
                "release.commit_message",
                module,
                version,
                entry,
            )?);
        }

        return match messages.len() {
            1 => messages.into_iter().next().ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    "release commit message was unexpectedly absent",
                )
            }),
            0 => Err(AppError::invalid_input(
                "release.commit_message",
                "a configured commit_message requires at least one versioned release in the member",
            )),
            _ => Err(AppError::invalid_input(
                "release.commit_message",
                "the configured commit_message renders differently for modules in one repository",
            )),
        };
    }
    let mut released = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in &plan.entries {
        if entry.planned_version.is_some() {
            // Modules sharing one collapsed tag contribute it once, in plan order.
            let name = planned_tag_name(entry)?;
            if seen.insert(name.to_string()) {
                released.push(name.to_string());
            }
        }
    }
    Ok(format!("release: {}", released.join(", ")))
}
