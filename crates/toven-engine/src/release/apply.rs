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
use toven_ports::{Artifact, ReleaseTarget, ReleaseVar, VcsReader, VcsWriter};

use super::publish::{self, PublishItem};
use super::{ReleasePlan, ReleaseStats, tag};

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
    push: bool,
    remote: String,
    branches: BTreeSet<String>,
    commit_message: Option<String>,
}

impl RepoReleaseSettings {
    /// Whether this repository pushes after accounting for CLI suppression.
    #[must_use]
    pub(crate) const fn pushes(&self, options: &ReleaseApplyOptions) -> bool {
        self.push && !options.no_push
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
    let message = commit_message(plan, &module_by_ref, targets, settings.commit_message())?;
    preflight_tags(plan, &module_by_ref, targets)?;

    // Pre-commit phase (undoable): apply mutations, then package every module.
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

    // Post-commit phase (no rollback): tag, optionally push, publish.
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_for(&module_by_ref, &entry.module)?;
            let target = target_for(targets, module)?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            let name = tag::format(&scheme, version);
            let message = tag_message(entry, module, version)?;
            writer.create_tag(&name, commit.as_str(), message.as_deref())?;
            stats.tagged_modules += 1;
        }
    }
    if settings.pushes(options) {
        let branch = reader.current_branch()?;
        writer.push(
            settings.remote(),
            &push_refspecs(plan, &module_by_ref, targets, &branch)?,
        )?;
    }

    if options.publish {
        let items = publish_items(plan, &module_by_ref, targets, &artifacts)?;
        publish::run(&items, options.retry_budget, &mut stats)?;
    }

    Ok(stats)
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

/// Apply every mutation and package every module, returning the artifacts keyed
/// by module. Runs entirely before the commit so the caller can restore the
/// working tree on failure.
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

    let mut artifacts = BTreeMap::new();
    for entry in &plan.entries {
        let module = module_for(module_by_ref, &entry.module)?;
        let target = target_for(targets, module)?;
        artifacts.insert(entry.module.clone(), target.package(module)?);
        stats.packaged_artifacts += 1;
    }
    Ok(artifacts)
}

/// Resolve the ordered publish items, skipping entries that need no publish.
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
    targets: &super::ReleaseTargets,
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
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_for(module_by_ref, &entry.module)?;
            let target = target_for(targets, module)?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            released.push(tag::format(&scheme, version));
        }
    }
    Ok(format!("release: {}", released.join(", ")))
}

fn preflight_tags(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &super::ReleaseTargets,
) -> AppResult<()> {
    for entry in &plan.entries {
        let Some(version) = &entry.planned_version else {
            continue;
        };
        let module = module_for(module_by_ref, &entry.module)?;
        let target = target_for(targets, module)?;
        target.tag_scheme(module, entry.tag_format.as_deref())?;
        tag_message(entry, module, version)?;
    }
    Ok(())
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

/// Refspecs pushed after tagging: the release commit's branch plus every
/// release tag.
///
/// The branch is pushed by its fully-qualified name (`refs/heads/<branch>`)
/// rather than `HEAD`: an ambiguous `HEAD` refspec depends on the remote's
/// `push.default` and silently fails to update the intended branch on a bare
/// remote, so the caller resolves the checked-out branch and pushes it
/// explicitly.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn push_refspecs(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &super::ReleaseTargets,
    branch: &str,
) -> AppResult<Vec<String>> {
    let mut refspecs = vec![format!("refs/heads/{branch}")];
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_for(module_by_ref, &entry.module)?;
            let target = target_for(targets, module)?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            refspecs.push(format!("refs/tags/{}", tag::format(&scheme, version)));
        }
    }
    Ok(refspecs)
}

#[cfg(test)]
mod tests {

    use rskit_errors::ErrorCode;
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{ChangeRecord, ChangeStatus, PublishOutcome, ReleaseMutation, TagScheme};
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, ReleaseCall, VcsWrite};

    use super::{ReleaseApplyOptions, reconcile_repo_settings, release_apply};
    use crate::release::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseEntry, ReleasePlan,
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
            push: true,
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
    fn configured_remote_and_push_gate_control_the_member_push() {
        let mut entry = entry("core", Version::new(1, 0, 0), true, 0);
        entry.remote = "release".into();
        entry.push = false;
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

        entry.push = true;
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
    fn configured_templates_render_commit_and_lightweight_tag() {
        let mut entry = entry("core", Version::new(1, 2, 3), true, 0);
        entry.commit_message = Some("release".into());
        let lightweight = entry.clone();
        let mut annotated = entry;
        annotated.module = mkey("app");
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
                second.push = false;
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
        let target = FakeReleaseTarget::new();

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
        assert_eq!(stats.tagged_modules, 1);
        assert_eq!(stats.published_modules, 0);
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Publish(_)))
        );
    }

    /// Build the `module_by_ref` map the push helper expects from a module set.
    fn module_by_ref(modules: &[Module]) -> std::collections::BTreeMap<ModuleKey, &Module> {
        modules
            .iter()
            .map(|module| (module.key(), module))
            .collect()
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
        let modules = [module("core")];
        let targets = targets(vec![("core", FakeReleaseTarget::new())]);
        let refspecs =
            super::push_refspecs(&plan, &module_by_ref(&modules), &targets, "release-train")
                .expect("refspecs");
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
        let modules = [module("core"), module("app")];

        // Resolve the branch exactly as the federated push does.
        let reader = RskitGitVcs::open(&work).expect("open reader");
        let branch = reader.current_branch().expect("current branch");
        assert_eq!(branch, "member-release");

        let targets = targets(vec![("core", FakeReleaseTarget::new())]);
        let refspecs = super::push_refspecs(&plan, &module_by_ref(&modules), &targets, &branch)
            .expect("refspecs");
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
