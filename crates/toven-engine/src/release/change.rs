//! Release change detection.
//!
//! Detection is member-scoped: each member repo contributes its own worktree
//! status, tag namespace, and configured baseline, while the changed seeds are
//! still resolved against the one federated umbrella graph. A single-repo
//! project is the N=1 degenerate member (no id, empty path prefix).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::AppResult;
use toven_model::{Module, ModuleKey};
use toven_ports::{BaselineSpec, ChangeRecord, CommitSummary, ReleaseAdapter, TagRef, TagScheme};

use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};
use crate::plan::PlanContext;

use super::{ReleaseBaseline, ReleaseTargets, ResolvedReleaseSettings, tag};

/// Per-module change-detection output.
#[derive(Debug, Clone)]
pub(super) struct ReleaseChanges {
    pub(super) changed: BTreeSet<ModuleKey>,
    pub(super) records: BTreeMap<ModuleKey, Vec<ChangeRecord>>,
    pub(super) commits: BTreeMap<ModuleKey, Vec<CommitSummary>>,
    pub(super) baselines: BTreeMap<ModuleKey, ReleaseBaseline>,
}

/// Detect modules changed since their release baseline across every member
/// repo.
///
/// # Errors
/// Propagates [`VcsReader`](toven_ports::VcsReader) failures (tag listing,
/// worktree status, changed-since).
pub(super) fn detect(
    context: &PlanContext,
    base_override: Option<&str>,
    readers: &MemberVcsReaders<'_>,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<ReleaseChanges> {
    let mut changes = ReleaseChanges {
        changed: BTreeSet::new(),
        records: BTreeMap::new(),
        commits: BTreeMap::new(),
        baselines: BTreeMap::new(),
    };
    for reader in readers.entries() {
        detect_member(
            context,
            base_override,
            reader,
            targets,
            settings,
            &mut changes,
        )?;
    }
    Ok(changes)
}

fn detect_member(
    context: &PlanContext,
    base_override: Option<&str>,
    reader: &MemberVcsReader<'_>,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    changes: &mut ReleaseChanges,
) -> AppResult<()> {
    let member = reader.member();
    let worktree = reader.umbrella_records(&reader.reader().worktree_status()?);
    // List every tag once: the VCS adapter enumerates all tags and filters
    // in-memory, so a per-module `list_tags(<glob>)` would re-scan the full tag set
    // for each module (O(modules × tags)). `tag::latest` parses and filters by the
    // module's prefix from this shared snapshot instead.
    let tags = reader.reader().list_tags(None)?;

    for module in context
        .federation
        .modules
        .iter()
        .filter(|module| module.member.as_ref() == member)
    {
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        let Some(target) = target_for(targets, module) else {
            continue;
        };
        let scheme = target.tag_scheme(module, resolved.tag_format.as_deref())?;
        let Some(spec) = baseline_spec(
            module,
            base_override,
            &tags,
            &scheme,
            &mut changes.baselines,
        ) else {
            changes.changed.insert(module.key());
            changes.records.insert(module.key(), Vec::new());
            changes.commits.insert(
                module.key(),
                collect_commits(reader, &changes.baselines, module)?,
            );
            continue;
        };
        let mut module_changes = reader.umbrella_records(&reader.reader().changed_since(&spec)?);
        module_changes.extend(worktree.iter().cloned());
        let seeds =
            crate::plan::changed_seeds(&module_changes, &context.graph, &context.federation);
        if seeds.contains(&module.key()) {
            changes.changed.insert(module.key());
            changes.records.insert(
                module.key(),
                crate::plan::changed_records_for_module(
                    module,
                    &module_changes,
                    &context.federation,
                ),
            );
            changes.commits.insert(
                module.key(),
                collect_commits(reader, &changes.baselines, module)?,
            );
        }
    }

    Ok(())
}

/// Collect the Conventional-Commit history a changed module's changelog is
/// generated from: the commits from the module's release baseline to `HEAD`,
/// scoped to the module's own directory.
///
/// A module with no baseline (a first release) has no prior tag to diff
/// against, so its whole path history is walked. Scoping by [`Module::root`]
/// keeps each module's changelog to its own changes; workspace-root noise
/// (lockfiles, CI config) attaches to no module.
fn collect_commits(
    reader: &MemberVcsReader<'_>,
    baselines: &BTreeMap<ModuleKey, ReleaseBaseline>,
    module: &Module,
) -> AppResult<Vec<CommitSummary>> {
    let since = baselines
        .get(&module.key())
        .and_then(|baseline| baseline.tag.as_deref());
    reader
        .reader()
        .commits_since(since, Some(module.root.as_path()))
}

/// Resolve the diff baseline for one module's release change detection.
///
/// A release baseline answers "what changed **since the last release**", so the
/// only baseline is the module's latest release tag. `--base` overrides the diff
/// ref explicitly *when a release tag exists*, while the tag continues to anchor
/// idempotency.
///
/// When no release tag exists the module has never been released, so `None` is
/// returned and the caller treats the module as an *initial release*: every
/// module is unreleased, and nothing has been published yet to diff against.
/// `--base` is deliberately **not** honored in that case, and neither is a
/// branch ref such as `[project].base_ref` — diffing a never-released module
/// against `origin/main` reports no changes on that branch and would silently
/// plan an empty first release.
fn baseline_spec(
    module: &Module,
    base_override: Option<&str>,
    tags: &[TagRef],
    scheme: &TagScheme,
    baselines: &mut BTreeMap<ModuleKey, ReleaseBaseline>,
) -> Option<BaselineSpec> {
    // The only baseline is the module's own latest release tag; it also carries
    // the version that offline idempotency anchors on.
    let Some((version, release_tag)) = tag::latest(scheme, tags) else {
        // No release tag: the module has never been released, so it is always an
        // initial release. `--base` is not honored here — a never-released module
        // has nothing to diff against, and letting a branch ref stand in would
        // silently plan an empty first release.
        baselines.insert(module.key(), ReleaseBaseline::initial(module.key()));
        return None;
    };

    baselines.insert(
        module.key(),
        ReleaseBaseline::tag(
            module.key(),
            release_tag.name.clone(),
            version,
            release_tag.target.clone(),
        ),
    );

    // `--base` overrides the diff ref (default: the release tag) while the tag
    // still anchors idempotency.
    let diff_ref = base_override.map_or_else(
        || release_tag.target.as_str().to_string(),
        ToString::to_string,
    );
    Some(BaselineSpec::explicit(diff_ref))
}

fn target_for<'a>(targets: &'a ReleaseTargets, module: &Module) -> Option<&'a dyn ReleaseAdapter> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
}
