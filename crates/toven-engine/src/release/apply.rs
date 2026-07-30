//! The release APPLY transaction: clean-tree guardrail, manifest mutation,
//! packaging, a single release commit, per-module tagging, optional push, and
//! the bounded publish loop.
//!
//! The transaction has a hard commit-success boundary. Everything before a
//! successful commit (mutation + packaging + attempted commit) is undoable: any
//! failure restores the working tree and creates no commit or tag. Tags,
//! optional push, and the publish loop run after that boundary and are **not**
//! rolled back — a publish failure surfaces as a typed error and the operator
//! resumes, relying on registry idempotency.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_util::Template;
use toven_model::{Module, ModuleKey};
use toven_ports::{Artifact, ReleaseCredentials, ReleaseTarget, ReleaseVar, VcsReader, VcsWriter};

use super::publish::{self, PublishItem};
use super::{PushPolicy, ReleasePlan, ReleaseStats};

/// Default rate-limit retry budget for the publish loop.
const DEFAULT_RETRY_BUDGET: usize = 5;

/// Runtime options for the release APPLY transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseApplyOptions {
    /// Suppress every config-permitted member push after tagging.
    pub no_push: bool,
    /// Publish the packaged artifacts to the registry after tagging. When
    /// false, the pipeline stops after commit/tag/push (the `release tag`
    /// surface).
    pub publish: bool,
    /// Maximum rate-limit retries per module in the publish loop.
    pub retry_budget: usize,
}

impl Default for ReleaseApplyOptions {
    fn default() -> Self {
        Self {
            no_push: true,
            publish: true,
            retry_budget: DEFAULT_RETRY_BUDGET,
        }
    }
}

/// Repository-scoped release settings reconciled from one member's plan entries.
///
/// A member release creates one commit and one push, so these settings cannot
/// vary among the modules it contains.
#[derive(Debug, Clone, Eq, PartialEq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct RepoReleaseSettings {
    push: PushPolicy,
    remote: String,
    branches: BTreeSet<String>,
    commit_message: Option<String>,
}

impl RepoReleaseSettings {
    /// Whether this repository pushes after accounting for CLI suppression.
    #[must_use]
    pub(crate) const fn pushes(&self, options: &ReleaseApplyOptions) -> bool {
        self.push.permits_push() && !options.no_push
    }

    /// Whether the release commit's branch is pushed alongside the tags.
    #[must_use]
    pub(crate) const fn pushes_branch(&self) -> bool {
        self.push.pushes_branch()
    }

    /// Configured remote selected for the repository push.
    #[must_use]
    pub(crate) fn remote(&self) -> &str {
        &self.remote
    }

    /// Configured release-branch allow-list.
    #[must_use]
    pub(crate) const fn branches(&self) -> &BTreeSet<String> {
        &self.branches
    }

    /// Configured release-commit template, if any.
    #[must_use]
    pub(crate) fn commit_message(&self) -> Option<&str> {
        self.commit_message.as_deref()
    }
}

/// Reconcile settings that govern a single commit/push from member plan entries.
///
/// # Errors
/// Returns a typed configuration error when modules in the same repository
/// disagree on a repository-scoped setting.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn reconcile_repo_settings(
    entries: &[super::ReleaseEntry],
) -> AppResult<RepoReleaseSettings> {
    let Some(first) = entries.first() else {
        return Err(AppError::new(
            ErrorCode::Internal,
            "cannot reconcile release settings for an empty repository plan",
        ));
    };
    let branches = first.branches.iter().cloned().collect::<BTreeSet<_>>();
    let settings = RepoReleaseSettings {
        push: first.push,
        remote: first.remote.clone(),
        branches,
        commit_message: first.commit_message.clone(),
    };
    for entry in entries.iter().skip(1) {
        if entry.push != settings.push {
            return repo_setting_conflict("push", first, entry);
        }
        if entry.remote != settings.remote {
            return repo_setting_conflict("remote", first, entry);
        }
        if entry.branches.iter().cloned().collect::<BTreeSet<_>>() != settings.branches {
            return repo_setting_conflict("branches", first, entry);
        }
        if entry.commit_message != settings.commit_message {
            return repo_setting_conflict("commit_message", first, entry);
        }
    }
    Ok(settings)
}

fn repo_setting_conflict(
    field: &str,
    first: &super::ReleaseEntry,
    conflicting: &super::ReleaseEntry,
) -> AppResult<RepoReleaseSettings> {
    Err(AppError::invalid_input(
        format!("release.{field}"),
        format!(
            "modules '{}' and '{}' resolve conflicting {field} settings in one repository",
            first.module, conflicting.module
        ),
    ))
}

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
    targets: &super::ReleaseTargets,
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
    // Resolve all pre-commit errors before mutating any manifest.
    preflight_targets(plan, &module_by_ref, targets)?;
    let message = commit_message(plan, &module_by_ref, settings.commit_message())?;

    // If every planned tag already exists, the git mutation phase already ran
    // and pushed on a prior attempt: resume by skipping manifest mutation,
    // commit, tag, and push, and let the idempotent publish and hosted-release
    // phases finish. A partial tag overlap has already failed closed above.
    if matches!(
        preflight_tags(plan, &module_by_ref, reader)?,
        TagPreflight::Resume
    ) {
        return resume_apply(plan, &module_by_ref, targets, options, stats);
    }

    // Pre-commit phase (undoable): apply mutations, then package every module
    // that will be published.
    let artifacts = match prepare(plan, &module_by_ref, targets, &mut stats) {
        Ok(artifacts) => artifacts,
        Err(error) => return Err(restore_or_precommit_error(writer, "prepare", error)),
    };

    // Commit boundary: if commit itself fails, no history was created yet, so the
    // pre-commit working tree mutations are still undoable.
    let commit = match writer.commit(&message) {
        Ok(commit) => commit,
        Err(error) => return Err(restore_or_precommit_error(writer, "commit", error)),
    };

    // Post-commit phase (no rollback): tag, optionally push, publish. A failure
    // here cannot undo the commit — it surfaces with forward-only recovery
    // guidance instead of pretending the run was atomic.
    let committed = || format!("release commit {} was created", commit.as_str());
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
    targets: &super::ReleaseTargets,
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
/// preflight, the commit message, and the pushed refspecs all anchor on this
/// single planned value instead of re-deriving the tag from the scheme, so a
/// run creates, validates, names, and pushes precisely the tag the plan showed
/// — no second computation that could drift. A planned-version entry always
/// carries a planned tag (both are resolved together during planning); the
/// typed error guards that invariant without a panic.
fn planned_tag_name(entry: &super::ReleaseEntry) -> AppResult<&str> {
    entry.planned_tag.as_deref().ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!(
                "module '{}' has a planned version but no planned tag; the release plan is \
                 internally inconsistent",
                entry.module
            ),
        )
    })
}

/// Create every planned release tag against the release commit.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn tag_releases(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    writer: &dyn VcsWriter,
    commit: &toven_ports::Oid,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    let mut created = BTreeSet::new();
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let name = planned_tag_name(entry)?;
            // A single-version workspace collapses many modules onto one shared
            // tag (`tag_format = "v{version}"`): that is one release train,
            // created once, not one tag per module. The hosted-release phase
            // collapses the same modules onto one hosted Release identically.
            if !created.insert(name.to_string()) {
                continue;
            }
            let module = module_for(module_by_ref, &entry.module)?;
            let message = tag_message(entry, module, version)?;
            writer.create_tag(name, commit.as_str(), message.as_deref())?;
            stats.tagged_modules += 1;
        }
    }
    Ok(())
}

/// Wrap a failure that happens after externally visible release state exists
/// (`state` says what) with forward-only recovery guidance, preserving the
/// original error code and cause. Past that point the run cannot be made to
/// look atomic: the operator inspects the partially released state and
/// forward-fixes — never rewrites or deletes published state.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn forward_recovery_error(state: &str, phase: &str, error: AppError) -> AppError {
    AppError::new(
        error.code(),
        format!(
            "release {phase} failed after {state}: {error}. Release tags, registry versions, \
             and hosted releases are immutable — inspect `toven release status`, resolve the \
             cause, preview again, and publish a forward fix; never rewrite or delete published \
             state"
        ),
    )
    .with_cause(error)
}

/// Reject a disallowed checked-out branch before release mutation.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn guard_release_branch(
    reader: &dyn VcsReader,
    branches: &BTreeSet<String>,
) -> AppResult<()> {
    if branches.is_empty() {
        return Ok(());
    }
    let branch = reader.current_branch()?;
    if branches.contains(&branch) {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "release.branches",
        format!(
            "checked-out branch '{branch}' is not allowed to cut this release (allowed: {})",
            branches.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
    ))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn restore_or_precommit_error(
    writer: &dyn VcsWriter,
    phase: &str,
    error: AppError,
) -> AppError {
    match writer.restore_worktree() {
        Ok(()) => error,
        Err(restore) => AppError::new(
            ErrorCode::Internal,
            format!(
                "release {phase} failed ({error}); additionally failed to restore worktree: {restore}"
            ),
        )
        .with_cause(error)
        .with_detail("restore_error", restore.to_string()),
    }
}

/// Reject a dirty working tree — the release transaction requires a clean tree.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn guard_clean_tree(reader: &dyn VcsReader) -> AppResult<()> {
    let status = reader.worktree_status()?;
    if status.is_empty() {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "release.worktree",
        format!(
            "the working tree has {} uncommitted change(s); commit or stash them before releasing",
            status.len()
        ),
    ))
}

/// Apply every mutation, then package every module that will be published,
/// returning the artifacts keyed by module. Runs entirely before the commit so
/// the caller can restore the working tree on failure.
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
    targets: &super::ReleaseTargets,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleKey, Artifact>> {
    for entry in &plan.entries {
        let module = module_for(module_by_ref, &entry.module)?;
        let target = target_for(targets, module)?;
        target.apply_release(module, &entry.mutation)?;
        stats.mutated_modules += 1;
    }

    package_publishable(plan, module_by_ref, targets, stats)
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
    targets: &super::ReleaseTargets,
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
    targets: &'a super::ReleaseTargets,
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
            credentials: ReleaseCredentials::new(entry.token_env.clone()),
        });
    }
    Ok(items)
}

fn module_for<'a>(
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

/// Pre-commit target preflight: every planned entry must resolve a release
/// target for its (member, ecosystem) pair. A member without a target fails
/// closed here, before any mutation, instead of being discovered mid-apply.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn preflight_targets(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &super::ReleaseTargets,
) -> AppResult<()> {
    for entry in &plan.entries {
        let module = module_for(module_by_ref, &entry.module)?;
        target_for(targets, module)?;
    }
    Ok(())
}

/// The pre-commit tag preflight verdict: whether a run is a fresh release or a
/// resume of an already-tagged one.
///
/// Release tags are immutable, so the set of planned tags that already exist on
/// the remote classifies the run: none is a normal apply; all is a resume (the
/// git mutation phase already ran and pushed, so it is skipped and only the
/// idempotent publish and hosted-release phases finish); a partial overlap is
/// an interrupted or divergent state that fails closed for a human forward fix.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum TagPreflight {
    /// No planned tag exists yet: apply the release normally.
    Fresh,
    /// Every planned tag already exists and the plan is internally consistent:
    /// resume by skipping manifest mutation, commit, tag, and push.
    Resume,
}

/// Pre-commit tag preflight: every planned tag scheme and annotation must
/// resolve, and no two modules in the plan may render the same tag with
/// divergent annotations. The set of planned tags that already exist on the
/// remote then classifies the run as [`Fresh`](TagPreflight::Fresh) (none
/// exist), [`Resume`](TagPreflight::Resume) (all exist), or a fail-closed
/// forward-fix conflict (a partial overlap). Release tags are immutable — a
/// partial set means an interrupted or divergent release a human must resolve,
/// never a tag this run may reuse or move.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn preflight_tags(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    reader: &dyn VcsReader,
) -> AppResult<TagPreflight> {
    let existing = reader.list_tags(None)?;
    let names: BTreeSet<&str> = existing.iter().map(|tag| tag.name.as_str()).collect();
    let planned = planned_tag_annotations(plan, module_by_ref)?;
    classify_planned_tags(&planned, &names)
}

/// Resolve every distinct planned tag with the annotation the first
/// contributing module renders, validating that modules sharing one tag agree
/// on its annotation.
///
/// A single-version workspace collapses many modules onto one shared tag
/// (`tag_format = "v{version}"`): that is one release train, tagged once, not a
/// per-module collision. Modules sharing a tag must agree on its annotation,
/// mirroring the hosted-release phase's shared-tag merge.
fn planned_tag_annotations(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
) -> AppResult<BTreeMap<String, Option<String>>> {
    let mut planned: BTreeMap<String, Option<String>> = BTreeMap::new();
    for entry in &plan.entries {
        let Some(version) = &entry.planned_version else {
            continue;
        };
        let module = module_for(module_by_ref, &entry.module)?;
        let annotation = tag_message(entry, module, version)?;
        let name = planned_tag_name(entry)?;
        if let Some(existing_annotation) = planned.get(name) {
            if existing_annotation != &annotation {
                return Err(AppError::invalid_input(
                    "release.tags",
                    format!(
                        "modules sharing release tag '{name}' disagree on the tag annotation; \
                         module '{}' renders a different tag_message — give the shared tag one \
                         annotation or a distinct tag_format",
                        entry.module
                    ),
                ));
            }
            continue;
        }
        planned.insert(name.to_string(), annotation);
    }
    Ok(planned)
}

/// Classify the planned tags against the tags already on the remote.
///
/// None present is a fresh apply; every planned tag present is a resume; a
/// partial overlap fails closed, because a subset of an immutable tag train
/// already existing is an interrupted or divergent release a human must
/// forward-fix, not a state this run may complete by reusing or moving a tag.
fn classify_planned_tags(
    planned: &BTreeMap<String, Option<String>>,
    existing: &BTreeSet<&str>,
) -> AppResult<TagPreflight> {
    let present: BTreeSet<&str> = planned
        .keys()
        .map(String::as_str)
        .filter(|name| existing.contains(name))
        .collect();
    if present.is_empty() {
        return Ok(TagPreflight::Fresh);
    }
    if present.len() == planned.len() {
        return Ok(TagPreflight::Resume);
    }
    let missing: Vec<&str> = planned
        .keys()
        .map(String::as_str)
        .filter(|name| !existing.contains(name))
        .collect();
    Err(AppError::invalid_input(
        "release.tags",
        format!(
            "a partial release tag set already exists: [{}] are present but [{}] are not; \
             release tags are immutable, so this interrupted or divergent release must be \
             forward-fixed with a new version rather than reusing or moving a tag",
            present.into_iter().collect::<Vec<_>>().join(", "),
            missing.join(", ")
        ),
    ))
}

fn render_template(
    template: &str,
    field: &str,
    module: &Module,
    version: &rskit_version::semver::Version,
    entry: &super::ReleaseEntry,
) -> AppResult<String> {
    let parsed = Template::parse(template, ReleaseVar::ALL).map_err(|error| {
        AppError::invalid_input(field, format!("invalid release template: {error}"))
            .with_cause(error)
    })?;
    parsed
        .render_with(|placeholder| match placeholder {
            ReleaseVar::Version => Ok(version.to_string()),
            ReleaseVar::Ecosystem => Ok(module.id.ecosystem.to_string()),
            ReleaseVar::Module => Ok(module.id.name.clone()),
            ReleaseVar::Channel => Ok(entry.prerelease_channel.clone().unwrap_or_default()),
            _ => Err(AppError::new(
                ErrorCode::Internal,
                "unknown release template placeholder",
            )),
        })
        .map_err(|error| {
            AppError::invalid_input(field, format!("failed to render release template: {error}"))
                .with_cause(error)
        })
}

/// Render one module's optional annotation template.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn tag_message(
    entry: &super::ReleaseEntry,
    module: &Module,
    version: &rskit_version::semver::Version,
) -> AppResult<Option<String>> {
    entry
        .tag_message
        .as_deref()
        .map(|template| render_template(template, "release.tag_message", module, version, entry))
        .transpose()
}

/// Refspecs pushed after tagging: the release commit's `branch` when it is
/// pushed (`Some`), plus every release tag.
///
/// The branch is pushed by its fully-qualified name (`refs/heads/<branch>`)
/// rather than `HEAD`: an ambiguous `HEAD` refspec depends on the remote's
/// `push.default` and silently fails to update the intended branch on a bare
/// remote, so the caller resolves the checked-out branch and pushes it
/// explicitly.
///
/// `None` selects the tags-only mode a protected branch requires, where the
/// release commit lands through a pull request rather than a direct branch
/// push: the branch ref is omitted, and because the branch name is never
/// needed the caller does not resolve it — a tags-only push also works from a
/// detached HEAD, the common CI checkout state.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn push_refspecs(plan: &ReleasePlan, branch: Option<&str>) -> AppResult<Vec<String>> {
    let mut refspecs = branch.map_or_else(Vec::new, |branch| vec![format!("refs/heads/{branch}")]);
    let mut seen = BTreeSet::new();
    for entry in &plan.entries {
        if entry.planned_version.is_some() {
            let name = planned_tag_name(entry)?;
            // Modules sharing one collapsed tag push a single tag refspec.
            if seen.insert(name.to_string()) {
                refspecs.push(format!("refs/tags/{name}"));
            }
        }
    }
    Ok(refspecs)
}

#[cfg(test)]
mod tests {

    use rskit_errors::ErrorCode;
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{
        ChangeRecord, ChangeStatus, Oid, PublishOutcome, ReleaseMutation, TagRef, TagScheme,
    };
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, ReleaseCall, VcsWrite};

    use super::{ReleaseApplyOptions, reconcile_repo_settings, release_apply};
    use crate::release::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseEntry, ReleasePlan,
    };
    use toven_ports::BumpLevel;

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    fn mkey(name: &str) -> ModuleKey {
        ModuleKey::bare(mref(name))
    }

    fn module(name: &str) -> Module {
        let mut module = Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap());
        module.manifest = Some(RepoPath::new(format!("crates/{name}/Cargo.toml")).unwrap());
        module
    }

    fn entry(name: &str, version: Version, publish_needed: bool, rank: usize) -> ReleaseEntry {
        ReleaseEntry {
            module: mkey(name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(version.clone()),
            planned_tag: Some(format!("rust/{name}@{version}")),
            level: BumpLevel::Patch,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(version),
            publication: if publish_needed {
                toven_ports::PublicationPolicy::Registry {
                    registry: "crates-io".into(),
                }
            } else {
                toven_ports::PublicationPolicy::TagOnly
            },
            publish_needed,
            tag_format: None,
            tag_message: None,
            commit_message: None,
            token_env: None,
            push: PushPolicy::BranchAndTags,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: rank,
            baseline: None,
            changelog: ChangelogEntry::new(mkey(name), "changed", Vec::new()),
        }
    }

    fn targets(pairs: Vec<(&str, FakeReleaseTarget)>) -> super::super::ReleaseTargets {
        // All fixtures use a single single-repo `rust` ecosystem.
        let mut map = super::super::ReleaseTargets::new();
        let (_, target) = pairs.into_iter().next().expect("at least one target");
        map.insert((None, EcosystemId::new("rust").unwrap()), Box::new(target));
        map
    }

    fn dirty() -> FakeVcsReader {
        FakeVcsReader::new()
            .with_worktree_status(vec![ChangeRecord::new("a.rs", ChangeStatus::Modified)])
    }

    #[test]
    fn applies_mutations_commits_tags_and_publishes_in_order() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", Version::new(0, 1, 1), true, 0),
                entry("app", Version::new(0, 1, 1), true, 1),
            ],
        );
        let modules = vec![module("core"), module("app")];
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        let stats = release_apply(
            &plan,
            &modules,
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect("release apply");

        assert_eq!(stats.mutated_modules, 2);
        assert_eq!(stats.packaged_artifacts, 2);
        assert_eq!(stats.tagged_modules, 2);
        assert_eq!(stats.published_modules, 2);
        assert_eq!(stats.skipped_published_modules, 0);

        let recorded = writer.writes();
        assert_eq!(
            recorded[0],
            VcsWrite::Commit("release: rust/core@0.1.1, rust/app@0.1.1".into())
        );
        assert!(matches!(
            &recorded[1],
            VcsWrite::CreateTag { name, target_rev, .. } if name == "rust/core@0.1.1" && target_rev == "c0ffee"
        ));
        assert!(
            matches!(&recorded[2], VcsWrite::CreateTag { name, .. } if name == "rust/app@0.1.1")
        );
        assert!(!recorded.iter().any(|w| matches!(w, VcsWrite::Push { .. })));

        // Publish happens after the commit/tag writes (apply -> package -> publish).
        let calls = target.calls();
        assert!(
            calls
                .iter()
                .filter(|c| matches!(c, ReleaseCall::ApplyRelease { .. }))
                .count()
                == 2
        );
        assert!(
            calls
                .iter()
                .filter(|c| matches!(c, ReleaseCall::Publish(_)))
                .count()
                == 2
        );
    }

    #[test]
    fn a_configured_token_env_is_threaded_to_the_publish_target() {
        // The resolved token_env rides from the plan entry to the release target
        // as the credential context — proving publish-time credential injection
        // is wired end-to-end (the target reads only the variable name).
        let mut core = entry("core", Version::new(0, 1, 1), true, 0);
        core.token_env = Some("CARGO_REGISTRY_TOKEN".into());
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core]);
        let target = FakeReleaseTarget::new();

        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new().with_commit_oid("c0ffee"),
            &ReleaseApplyOptions::default(),
        )
        .expect("release apply");

        assert_eq!(
            target.publish_token_envs(),
            vec![Some("CARGO_REGISTRY_TOKEN".to_string())],
            "the publish target must receive the resolved token_env as its credential context"
        );
    }

    #[test]
    fn push_emits_commit_and_tag_refspecs() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(1, 0, 0), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("release apply with push");

        let (remote, push) = writer
            .writes()
            .into_iter()
            .find_map(|w| match w {
                VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
                _ => None,
            })
            .expect("push recorded");
        assert_eq!(remote, "origin");
        assert_eq!(
            push,
            vec![
                "refs/heads/main".to_string(),
                "refs/tags/rust/core@1.0.0".to_string()
            ]
        );
    }

    #[test]
    fn tags_only_push_policy_pushes_tags_only() {
        let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
        entry.push = PushPolicy::TagsOnly;
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]);
        let writer = FakeVcsWriter::new();

        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("release apply with tags-only push");

        let (_, push) = writer
            .writes()
            .into_iter()
            .find_map(|w| match w {
                VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
                _ => None,
            })
            .expect("push recorded");
        assert_eq!(push, vec!["refs/tags/rust/core@1.0.0".to_string()]);
    }

    #[test]
    fn push_refspecs_omits_the_branch_when_no_branch_is_pushed() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(1, 0, 0), true, 0)],
        );

        let with_branch = super::push_refspecs(&plan, Some("main")).expect("refspecs");
        assert_eq!(with_branch[0], "refs/heads/main");

        let tags_only = super::push_refspecs(&plan, None).expect("refspecs");
        assert!(
            tags_only.iter().all(|spec| spec.starts_with("refs/tags/")),
            "{tags_only:?}"
        );
        assert_eq!(tags_only, vec!["refs/tags/rust/core@1.0.0".to_string()]);
    }

    #[test]
    fn tags_only_push_proceeds_on_a_detached_head() {
        let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
        entry.push = PushPolicy::TagsOnly;
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]);
        let writer = FakeVcsWriter::new();

        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new().with_detached_head(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("a tags-only push does not require a checked-out branch");

        let (_, push) = writer
            .writes()
            .into_iter()
            .find_map(|w| match w {
                VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
                _ => None,
            })
            .expect("push recorded");
        assert_eq!(push, vec!["refs/tags/rust/core@1.0.0".to_string()]);
    }

    #[test]
    fn branch_push_still_requires_a_checked_out_branch() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(1, 0, 0), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new().with_detached_head(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect_err("pushing the branch requires resolving one");

        assert!(error.to_string().contains("detached"), "{error}");
    }

    #[test]
    fn reconcile_rejects_conflicting_push_policies() {
        let first = entry("core", Version::new(1, 0, 0), true, 0);
        let mut second = entry("util", Version::new(1, 0, 0), true, 1);
        second.push = PushPolicy::TagsOnly;

        let error = reconcile_repo_settings(&[first, second]).expect_err("conflict rejected");
        assert!(error.to_string().contains("push"), "{error}");
    }

    #[test]
    fn configured_remote_and_push_gate_control_the_member_push() {
        let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
        entry.remote = "release".into();
        entry.push = PushPolicy::Disabled;
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry.clone()]);
        let writer = FakeVcsWriter::new();

        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("config-gated local release");
        assert!(
            !writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::Push { .. }))
        );

        entry.push = PushPolicy::BranchAndTags;
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]);
        let writer = FakeVcsWriter::new();
        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("configured remote push");
        assert!(writer.writes().iter().any(|write| matches!(
            write,
            VcsWrite::Push { remote, .. } if remote == "release"
        )));
    }

    #[test]
    fn branch_restriction_rejects_before_any_release_write() {
        let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
        entry.branches = vec!["release".into()];
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]),
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new().with_current_branch("main"),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("disallowed branch");

        assert!(error.to_string().contains("release.branches"));
        assert!(writer.writes().is_empty());
    }

    #[test]
    fn two_modules_sharing_a_tag_collapse_into_a_single_release_train() {
        let mut core = entry("core", Version::new(0, 2, 0), true, 0);
        core.tag_format = Some("v{version}".into());
        // Plan-time tag resolution renders both modules to the same tag: a
        // single-version workspace collapses onto one shared repository tag.
        core.planned_tag = Some("v0.2.0".into());
        let mut app = entry("app", Version::new(0, 2, 0), true, 1);
        app.tag_format = Some("v{version}".into());
        app.planned_tag = Some("v0.2.0".into());
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core, app]);
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        let stats = release_apply(
            &plan,
            &[module("core"), module("app")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("modules sharing a tag collapse into one release train");

        // The shared tag is created exactly once for the whole train.
        assert_eq!(stats.tagged_modules, 1);
        let create_tags: Vec<_> = writer
            .writes()
            .into_iter()
            .filter_map(|w| match w {
                VcsWrite::CreateTag { name, .. } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(create_tags, vec!["v0.2.0".to_string()]);

        // The commit message lists the collapsed tag once, and the push carries
        // a single tag refspec.
        let recorded = writer.writes();
        assert_eq!(recorded[0], VcsWrite::Commit("release: v0.2.0".into()));
        let tag_refspecs: Vec<_> = recorded
            .iter()
            .filter_map(|w| match w {
                VcsWrite::Push { refspecs, .. } => Some(refspecs.clone()),
                _ => None,
            })
            .flatten()
            .filter(|r| r.starts_with("refs/tags/"))
            .collect();
        assert_eq!(tag_refspecs, vec!["refs/tags/v0.2.0".to_string()]);
    }

    #[test]
    fn modules_sharing_a_tag_with_divergent_annotations_are_rejected() {
        let mut core = entry("core", Version::new(0, 2, 0), true, 0);
        core.tag_format = Some("v{version}".into());
        core.planned_tag = Some("v0.2.0".into());
        core.tag_message = Some("core annotation".into());
        let mut app = entry("app", Version::new(0, 2, 0), true, 1);
        app.tag_format = Some("v{version}".into());
        app.planned_tag = Some("v0.2.0".into());
        app.tag_message = Some("app annotation".into());
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core, app]);
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core"), module("app")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a shared tag with conflicting annotations must fail closed");

        let message = error.to_string();
        assert!(message.contains("v0.2.0"), "{message}");
        assert!(message.contains("annotation"), "{message}");
        assert!(writer.writes().is_empty());
    }

    #[test]
    fn a_module_without_a_release_target_is_rejected_before_any_mutation() {
        // The go module has no registered target; the failure must surface
        // before the rust module's mutation, not inside `prepare`.
        let go_ref = ModuleRef::new(EcosystemId::new("go").unwrap(), "cache-redis").unwrap();
        let mut go_module = Module::new(go_ref.clone(), RepoPath::new("cache/redis").unwrap());
        go_module.manifest = Some(RepoPath::new("cache/redis/go.mod").unwrap());
        let mut go_entry = entry("core", Version::new(2, 0, 0), true, 1);
        go_entry.module = ModuleKey::bare(go_ref);
        go_entry.planned_tag = Some("cache/redis/v2.0.0".into());

        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0), go_entry],
        );
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core"), go_module],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a missing release target must fail closed before mutation");

        assert!(error.to_string().contains("has no release target"));
        assert!(writer.writes().is_empty(), "no VCS write may happen");
        assert!(
            target.calls().is_empty(),
            "no target mutation/package may happen: {:?}",
            target.calls()
        );
    }

    #[test]
    fn a_publish_failure_after_the_commit_carries_forward_only_recovery_guidance() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 2, 0), true, 0)],
        );
        let target = FakeReleaseTarget::new().with_publish_failure("registry unavailable");
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a post-commit publish failure must surface recovery guidance");

        let message = error.to_string();
        assert!(message.contains("publication"), "{message}");
        assert!(message.contains("registry unavailable"), "{message}");
        assert!(message.contains("toven release status"), "{message}");
        assert!(message.contains("forward fix"), "{message}");
        // The commit is past the rollback boundary: it happened, and no
        // worktree restore may be attempted for a post-commit failure.
        assert!(
            writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::Commit(_)))
        );
        assert!(
            !writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::RestoreWorktree))
        );
    }

    #[test]
    fn a_push_failure_after_the_commit_carries_forward_only_recovery_guidance() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 2, 0), true, 0)],
        );
        let writer = FakeVcsWriter::new().with_push_failure("remote rejected");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect_err("a post-commit push failure must surface recovery guidance");

        let message = error.to_string();
        assert!(message.contains("push"), "{message}");
        assert!(message.contains("remote rejected"), "{message}");
        assert!(message.contains("toven release status"), "{message}");
        assert!(message.contains("forward fix"), "{message}");
        // The commit and tag are past the rollback boundary: no worktree
        // restore may be attempted for a post-commit push failure.
        assert!(
            writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::Commit(_)))
        );
        assert!(
            !writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::RestoreWorktree))
        );
    }

    #[test]
    fn an_all_tags_exist_release_resumes_without_git_mutation() {
        // The planned tag already exists on the remote: the commit, tag, and
        // push happened on a prior attempt, so APPLY resumes — no manifest
        // mutation, commit, tag, or push — and the version is already published,
        // so the publish loop is a clean no-op.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 2, 0), false, 0)],
        );
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new();
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &reader,
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect("an already-tagged release resumes rather than failing closed");

        assert!(stats.resumed, "the run is marked resumed");
        assert_eq!(stats.tagged_modules, 0);
        assert!(
            writer.writes().is_empty(),
            "no commit/tag/push may happen on resume: {:?}",
            writer.writes()
        );
        assert!(
            !target.calls().iter().any(|call| matches!(
                call,
                ReleaseCall::ApplyRelease { .. }
                    | ReleaseCall::Package(_)
                    | ReleaseCall::Publish(_)
            )),
            "no manifest mutation, packaging, or publish may happen on resume: {:?}",
            target.calls()
        );
    }

    #[test]
    fn a_resume_publishes_a_version_the_registry_still_lacks() {
        // The planned tag exists (commit, tag, and push happened on a prior
        // attempt) but the registry publish never completed: the entry is still
        // publish-needed. A resume must package it (no manifest mutation) and
        // publish it, completing the interrupted publish rather than failing on a
        // missing artifact.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 2, 0), true, 0)],
        );
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new();
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &reader,
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect("a resume completes the interrupted publish");

        assert!(stats.resumed, "the run is marked resumed");
        assert_eq!(stats.packaged_artifacts, 1);
        assert_eq!(stats.published_modules, 1);
        assert_eq!(stats.tagged_modules, 0);
        assert!(
            writer.writes().is_empty(),
            "no commit/tag/push may happen on resume: {:?}",
            writer.writes()
        );
        assert!(
            !target
                .calls()
                .iter()
                .any(|call| matches!(call, ReleaseCall::ApplyRelease { .. })),
            "a resume never mutates a manifest: {:?}",
            target.calls()
        );
        assert!(
            target
                .calls()
                .iter()
                .any(|call| matches!(call, ReleaseCall::Package(_)))
                && target
                    .calls()
                    .iter()
                    .any(|call| matches!(call, ReleaseCall::Publish(_))),
            "a resume packages and publishes the missing version: {:?}",
            target.calls()
        );
    }

    #[test]
    fn a_partial_planned_tag_set_is_rejected_before_any_mutation() {
        // One of two planned tags exists: an interrupted or divergent release,
        // never a resume — it fails closed with immutable/forward-fix guidance
        // before any mutation.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", Version::new(0, 2, 0), true, 0),
                entry("app", Version::new(0, 2, 0), true, 1),
            ],
        );
        let writer = FakeVcsWriter::new();
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("deadbee"))]);

        let error = release_apply(
            &plan,
            &[module("core"), module("app")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &reader,
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a partial tag overlap must fail closed before mutation");

        let message = error.to_string();
        assert!(message.contains("rust/core@0.2.0"), "{message}");
        assert!(message.contains("rust/app@0.2.0"), "{message}");
        assert!(message.contains("immutable"), "{message}");
        assert!(message.contains("forward-fix"), "{message}");
        assert!(writer.writes().is_empty(), "no VCS write may happen");
    }

    #[test]
    fn configured_templates_render_commit_and_lightweight_tag() {
        let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
        entry.commit_message = Some("release".into());
        let lightweight = entry.clone();
        let mut annotated = entry;
        annotated.module = mkey("app");
        annotated.planned_tag = Some("rust/app@1.2.3".into());
        annotated.tag_message = Some("tag {ecosystem}/{module} {version}".into());
        let writer = FakeVcsWriter::new();

        release_apply(
            &ReleasePlan::new(BumpPolicy::SemverCascade, vec![lightweight, annotated]),
            &[module("core"), module("app")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect("template release");

        let recorded = writer.writes();
        assert!(matches!(
            &recorded[0],
            VcsWrite::Commit(message) if message == "release"
        ));
        assert!(matches!(
            &recorded[1],
            VcsWrite::CreateTag { message: None, .. }
        ));
        assert!(matches!(
            &recorded[2],
            VcsWrite::CreateTag { message: Some(message), .. }
                if message == "tag rust/app 1.2.3"
        ));
    }

    #[test]
    fn invalid_commit_template_does_not_mutate_or_restore() {
        let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
        entry.commit_message = Some("{invalid}".into());
        let writer = FakeVcsWriter::new();
        let target = FakeReleaseTarget::new();

        let error = release_apply(
            &ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry]),
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("invalid commit template");

        assert!(error.to_string().contains("release.commit_message"));
        assert!(writer.writes().is_empty());
        assert!(target.calls().is_empty());
    }

    #[test]
    fn repository_scoped_settings_must_agree_between_modules() {
        let first = entry("core", Version::new(1, 0, 0), true, 0);
        let cases = [
            ("push", {
                let mut second = entry("app", Version::new(1, 0, 0), true, 1);
                second.push = PushPolicy::Disabled;
                second
            }),
            ("remote", {
                let mut second = entry("app", Version::new(1, 0, 0), true, 1);
                second.remote = "release".into();
                second
            }),
            ("branches", {
                let mut second = entry("app", Version::new(1, 0, 0), true, 1);
                second.branches = vec!["release".into()];
                second
            }),
            ("commit_message", {
                let mut second = entry("app", Version::new(1, 0, 0), true, 1);
                second.commit_message = Some("release".into());
                second
            }),
        ];
        for (field, second) in cases {
            let error = reconcile_repo_settings(&[first.clone(), second])
                .expect_err("conflicting repository setting");
            assert!(
                error.to_string().contains(&format!("release.{field}")),
                "{error}"
            );
        }
    }

    #[test]
    fn tag_only_run_commits_and_tags_without_publishing() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                publish: false,
                ..Default::default()
            },
        )
        .expect("tag-only release apply");

        assert_eq!(stats.tagged_modules, 1);
        assert_eq!(stats.published_modules, 0);
        assert!(
            writer.writes().iter().any(
                |w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "rust/core@0.1.1")
            )
        );
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Publish(_))),
            "tag-only run must not publish"
        );
    }

    #[test]
    fn mixed_ecosystem_umbrella_tags_each_member_with_its_own_scheme() {
        // A Rust crate (crates.io tag grammar) and a Go module (path-based git tag
        // grammar) release over the one topological order, each carrying its own
        // target-owned tag scheme.
        let go_ref = ModuleRef::new(EcosystemId::new("go").unwrap(), "cache-redis").unwrap();
        let go_key = ModuleKey::bare(go_ref.clone());
        let mut go_module = Module::new(go_ref, RepoPath::new("cache/redis").unwrap());
        go_module.manifest = Some(RepoPath::new("cache/redis/go.mod").unwrap());
        let mut go_entry = entry("core", Version::new(2, 0, 0), true, 1);
        go_entry.module = go_key;
        // Plan-time tag resolution uses the go target's path-based scheme.
        go_entry.planned_tag = Some("cache/redis/v2.0.0".into());

        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0), go_entry],
        );

        let go_target =
            FakeReleaseTarget::new().with_tag_scheme(TagScheme::new("cache/redis/v", ""));
        let mut map = super::super::ReleaseTargets::new();
        map.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(FakeReleaseTarget::new()),
        );
        map.insert((None, EcosystemId::new("go").unwrap()), Box::new(go_target));
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        release_apply(
            &plan,
            &[module("core"), go_module],
            &map,
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                publish: false,
                ..Default::default()
            },
        )
        .expect("mixed-ecosystem release apply");

        let recorded = writer.writes();
        assert_eq!(
            recorded[0],
            VcsWrite::Commit("release: rust/core@0.1.1, cache/redis/v2.0.0".into())
        );
        assert!(
            recorded.iter().any(
                |w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "rust/core@0.1.1")
            ),
            "rust member keeps its crates.io tag grammar"
        );
        assert!(
            recorded.iter().any(
                |w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "cache/redis/v2.0.0")
            ),
            "go member uses its path-based git tag grammar"
        );
    }

    #[test]
    fn restores_worktree_when_commit_fails() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new().with_commit_failure("commit failed");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("commit failure must surface");

        assert!(error.to_string().contains("commit failed"));
        assert_eq!(
            writer.writes(),
            vec![
                VcsWrite::Commit("release: rust/core@0.1.1".into()),
                VcsWrite::RestoreWorktree
            ]
        );
    }

    #[test]
    fn prepare_failure_reports_restore_failure_without_losing_original_error() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new().with_restore_failure("restore failed");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![(
                "core",
                FakeReleaseTarget::new().with_package_failure("package failed"),
            )]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("prepare and restore failures must surface together");

        let message = error.to_string();
        assert!(message.contains("release prepare failed"));
        assert!(message.contains("package failed"));
        assert!(message.contains("restore failed"));
        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(
            error
                .cause()
                .is_some_and(|cause| cause.to_string().contains("package failed"))
        );
        assert_eq!(writer.writes(), vec![VcsWrite::RestoreWorktree]);
    }

    #[test]
    fn commit_failure_reports_restore_failure_without_losing_original_error() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new()
            .with_commit_failure("commit failed")
            .with_restore_failure("restore failed");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("commit and restore failures must surface together");

        let message = error.to_string();
        assert!(message.contains("release commit failed"));
        assert!(message.contains("commit failed"));
        assert!(message.contains("restore failed"));
        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(
            error
                .cause()
                .is_some_and(|cause| cause.to_string().contains("commit failed"))
        );
        assert_eq!(
            writer.writes(),
            vec![
                VcsWrite::Commit("release: rust/core@0.1.1".into()),
                VcsWrite::RestoreWorktree
            ]
        );
    }

    #[test]
    fn dirty_worktree_is_rejected() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &dirty(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("dirty worktree must be rejected");
        assert!(error.to_string().contains("uncommitted change"));
        assert!(
            writer.writes().is_empty(),
            "no writes on a tripped guardrail"
        );
    }

    #[test]
    fn no_option_bypasses_the_clean_tree_guardrail() {
        // The clean-tree guardrail has no bypass: a dirty tree is always rejected,
        // regardless of options. This regression-tests the removal of `--allow-dirty`.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        for options in [
            ReleaseApplyOptions::default(),
            ReleaseApplyOptions {
                no_push: false,
                publish: true,
                ..ReleaseApplyOptions::default()
            },
        ] {
            let writer = FakeVcsWriter::new();
            let error = release_apply(
                &plan,
                &[module("core")],
                &targets(vec![("core", FakeReleaseTarget::new())]),
                &dirty(),
                &writer,
                &options,
            )
            .expect_err("dirty worktree must always be rejected");
            assert!(error.to_string().contains("uncommitted change"));
        }
    }

    #[test]
    fn package_failure_rolls_back_before_commit() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![(
                "core",
                FakeReleaseTarget::new().with_package_failure("boom"),
            )]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("package failure surfaces");
        assert!(error.to_string().contains("boom"));

        let recorded = writer.writes();
        assert_eq!(recorded, vec![VcsWrite::RestoreWorktree]);
        assert!(!recorded.iter().any(|w| matches!(w, VcsWrite::Commit(_))));
    }

    #[test]
    fn already_published_version_is_pre_skipped() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("pre-skip");

        assert_eq!(stats.published_modules, 0);
        assert_eq!(stats.skipped_published_modules, 1);
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Publish(_))),
            "an already-published version must not be re-published"
        );
    }

    #[test]
    fn already_published_outcome_is_resume_safe() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target =
            FakeReleaseTarget::new().with_publish_outcome(PublishOutcome::AlreadyPublished);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("resume-safe already-published");

        assert_eq!(stats.published_modules, 0);
        assert_eq!(stats.skipped_published_modules, 1);
    }

    #[test]
    fn rate_limited_publish_retries_within_budget() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new().with_publish_outcomes(vec![
            PublishOutcome::RateLimited { retry_after: None },
            PublishOutcome::Published,
        ]);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("retry then publish");

        assert_eq!(stats.published_modules, 1);
        assert_eq!(stats.rate_limited_waits, 1);
    }

    #[test]
    fn rate_limited_publish_surfaces_exhausted_budget() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new()
            .with_publish_outcome(PublishOutcome::RateLimited { retry_after: None });

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions {
                retry_budget: 2,
                ..Default::default()
            },
        )
        .expect_err("exhausted budget surfaces");
        assert!(error.to_string().contains("rate-limit retry budget"));
    }

    #[test]
    fn entries_without_publish_needed_are_not_published() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), false, 0)],
        );
        // A tag-only module is never packaged: `cargo package` on an unpublished
        // workspace crate cannot resolve its intra-workspace deps from the
        // registry and exits non-zero. A package attempt here would fail the
        // whole tag-only release, so wire the double to blow up if it happens.
        let target = FakeReleaseTarget::new().with_package_failure("tag-only must not package");

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("apply without publish");

        assert_eq!(stats.mutated_modules, 1);
        assert_eq!(stats.packaged_artifacts, 0);
        assert_eq!(stats.tagged_modules, 1);
        assert_eq!(stats.published_modules, 0);
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Package(_) | ReleaseCall::Publish(_)))
        );
    }

    #[test]
    fn standalone_push_lands_named_branch_and_tags_on_a_real_bare_remote() {
        use crate::vcs::RskitGitVcs;
        use rskit_git::RefManager;
        use toven_ports::VcsWriter;
        use toven_testkit::TestWorkspace;
        use toven_testkit::git::{GitScenario, ref_map_at};

        let workspace = TestWorkspace::new("release-standalone-real-push");
        let work = workspace.child("work").expect("work dir");
        let bare = workspace.child("remote.git").expect("bare dir");

        // A real working repo, committed on a named, non-`main` branch.
        let scenario = GitScenario::init(&work).expect("init work");
        scenario
            .commit_file("Cargo.toml", "name=core\n", "import")
            .expect("initial commit");
        scenario
            .branch_and_checkout("release-train")
            .expect("named branch");
        GitScenario::init_bare(&bare).expect("init bare remote");
        scenario.add_remote("origin", &bare).expect("wire remote");

        // Lightweight release tag the plan pushes (target oid == commit oid, so
        // local and remote ref-maps compare directly with no peel ambiguity).
        scenario
            .repo()
            .create_tag("rust/core@1.0.0", "HEAD", None)
            .expect("release tag");
        let local = scenario.ref_map().expect("local refs");

        // The exact refspecs the standalone push uses for this branch.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(1, 0, 0), true, 0)],
        );
        let refspecs = super::push_refspecs(&plan, Some("release-train")).expect("refspecs");
        assert_eq!(
            refspecs,
            vec![
                "refs/heads/release-train".to_string(),
                "refs/tags/rust/core@1.0.0".to_string(),
            ]
        );

        // Push through the real rskit-git-backed writer to the real bare remote.
        RskitGitVcs::open(&work)
            .expect("open work")
            .push("origin", &refspecs)
            .expect("push to bare remote");

        // The bare remote received exactly the named branch and the tag, at the
        // same oids as the local repo — the `HEAD` refspec never created these.
        let remote = ref_map_at(&bare).expect("remote refs");
        assert_eq!(
            remote.get("refs/heads/release-train"),
            local.get("refs/heads/release-train"),
        );
        assert_eq!(
            remote.get("refs/tags/rust/core@1.0.0"),
            local.get("refs/tags/rust/core@1.0.0"),
        );
        assert!(!remote.contains_key("refs/heads/HEAD"));
        assert!(!remote.contains_key("refs/heads/main"));
    }

    #[test]
    fn federated_style_multi_module_push_lands_every_tag_on_a_real_bare_remote() {
        // The federated member push (federation::release::commit_member_shard)
        // shares `push_refspecs` and the same rskit-git writer as the standalone
        // path, adding only `reader().current_branch()`. This proves that shared
        // mechanism pushes the resolved branch plus every module tag to a real
        // custom-named remote for a multi-module member shard.
        use crate::vcs::RskitGitVcs;
        use rskit_git::RefManager;
        use toven_ports::{VcsReader, VcsWriter};
        use toven_testkit::TestWorkspace;
        use toven_testkit::git::{GitScenario, ref_map_at};

        let workspace = TestWorkspace::new("release-federated-real-push");
        let work = workspace.child("work").expect("work dir");
        let bare = workspace.child("upstream.git").expect("bare dir");

        let scenario = GitScenario::init(&work).expect("init work");
        scenario
            .commit_file("Cargo.toml", "name=member\n", "import")
            .expect("initial commit");
        scenario
            .branch_and_checkout("member-release")
            .expect("named branch");
        GitScenario::init_bare(&bare).expect("init bare remote");
        scenario.add_remote("upstream", &bare).expect("wire remote");

        for tag in ["rust/core@1.0.0", "rust/app@1.0.0"] {
            scenario
                .repo()
                .create_tag(tag, "HEAD", None)
                .expect("release tag");
        }
        let local = scenario.ref_map().expect("local refs");

        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", Version::new(1, 0, 0), true, 0),
                entry("app", Version::new(1, 0, 0), true, 1),
            ],
        );

        // Resolve the branch exactly as the federated push does.
        let reader = RskitGitVcs::open(&work).expect("open reader");
        let branch = reader.current_branch().expect("current branch");
        assert_eq!(branch, "member-release");

        let refspecs = super::push_refspecs(&plan, Some(&branch)).expect("refspecs");
        assert_eq!(
            refspecs,
            vec![
                "refs/heads/member-release".to_string(),
                "refs/tags/rust/core@1.0.0".to_string(),
                "refs/tags/rust/app@1.0.0".to_string(),
            ]
        );

        RskitGitVcs::open(&work)
            .expect("open writer")
            .push("upstream", &refspecs)
            .expect("push to bare remote");

        let remote = ref_map_at(&bare).expect("remote refs");
        for refname in [
            "refs/heads/member-release",
            "refs/tags/rust/core@1.0.0",
            "refs/tags/rust/app@1.0.0",
        ] {
            assert_eq!(remote.get(refname), local.get(refname), "{refname}");
        }
        assert!(!remote.contains_key("refs/heads/HEAD"));
    }
}
