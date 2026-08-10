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

use toven_engine_core::federation::baseline::{MemberVcsReader, MemberVcsReaders};
use toven_engine_core::plan::PlanContext;

use crate::model::BaselineSource;
use crate::{ReleaseBaseline, ReleaseTargets, ResolvedReleaseSettings};

/// Per-module change-detection output.
#[derive(Debug, Clone)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct ReleaseChanges {
    pub(crate) changed: BTreeSet<ModuleKey>,
    pub(crate) records: BTreeMap<ModuleKey, Vec<ChangeRecord>>,
    pub(crate) commits: BTreeMap<ModuleKey, Vec<CommitSummary>>,
    pub(crate) baselines: BTreeMap<ModuleKey, ReleaseBaseline>,
}

/// Detect modules changed since their release baseline across every member
/// repo.
///
/// # Errors
/// Propagates [`VcsReader`](toven_ports::VcsReader) failures (tag listing,
/// worktree status, changed-since).
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn detect(
    context: &PlanContext,
    base_override: Option<&str>,
    readers: &MemberVcsReaders<'_>,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    intent: crate::versioning::bump::CutIntent,
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
            intent,
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
    intent: crate::versioning::bump::CutIntent,
    changes: &mut ReleaseChanges,
) -> AppResult<()> {
    let member = reader.member();
    let worktree = reader.umbrella_records(&reader.reader().worktree_status()?);
    // List every tag once: the VCS adapter enumerates all tags and filters
    // in-memory, so a per-module `list_tags(<glob>)` would re-scan the full tag set
    // for each module (O(modules × tags)). The baseline resolver parses and filters
    // by the module's prefix from this shared snapshot instead.
    let tags = reader.reader().list_tags(None)?;

    // Version-reference files (READMEs/docs whose pins `bump` rewrites) are
    // downstream artifacts of the version decision, not inputs to it: their only
    // expected diff is a synced version token. Filtering them from the seed set
    // keeps a synced-only file from re-triggering a bump — the native mirror of
    // rskit's tool-generated-change filter. The set is repo-scoped (the union of
    // every module's declared references) and empty for a project that declares
    // none, so detection is unchanged there.
    let reference_globs = version_reference_globs(settings);

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
            scheme,
            target,
            &mut changes.baselines,
        )?
        else {
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
        if !reference_globs.is_empty() {
            module_changes
                .retain(|record| !path_matches_any(&reference_globs, record.path.as_path()));
        }
        let seeds = toven_engine_core::plan::changed_seeds(
            &module_changes,
            &context.graph,
            &context.federation,
        );
        // A maintainer-owned module is force-included only on the
        // verify-and-publish path: the maintainer already cut the tag/Release at
        // the declared manifest version, so `plan`/`publish` must reach it to
        // verify that tag and publish idempotently against it. The `bump` path
        // must NOT force-include it — a maintainer-owned module that is not ahead
        // of its baseline has nothing to advance, so an all-maintainer-owned
        // workspace with no changes yields an empty bump plan (a genuine no-op).
        let force_maintainer =
            intent.forces_maintainer_owned() && resolved.entrypoint.is_maintainer_owned();
        if seeds.contains(&module.key()) || force_maintainer {
            changes.changed.insert(module.key());
            changes.records.insert(
                module.key(),
                toven_engine_core::plan::changed_records_for_module(
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
/// A release baseline answers "what changed **since the last release**". This
/// step anchors on the module's own latest release tag via the shared
/// [`resolve_baseline`] resolver with [`BaselineSource::OwnTag`] — the
/// behavior-preserving default; the registry/umbrella-anchored sources are
/// wired into detection in a later step. `--base` overrides the diff ref
/// explicitly *when a release tag exists*, while the tag continues to anchor
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
    scheme: TagScheme,
    version_source: &dyn toven_ports::VersionSource,
    baselines: &mut BTreeMap<ModuleKey, ReleaseBaseline>,
) -> AppResult<Option<BaselineSpec>> {
    let baseline = crate::versioning::baseline::resolve_baseline(
        module,
        &BaselineSource::own_tag(scheme),
        version_source,
        tags,
    )?;

    // No anchor at all: the module has never been released, so it is always an
    // initial release. `--base` is not honored here — a never-released module
    // has nothing to diff against, and letting a branch ref stand in would
    // silently plan an empty first release.
    if baseline.is_initial() {
        baselines.insert(module.key(), baseline);
        return Ok(None);
    }

    // `--base` overrides the diff ref (default: the release tag commit) while the
    // tag still anchors idempotency.
    let diff_ref = base_override.map_or_else(
        || {
            baseline
                .target
                .as_ref()
                .map_or_else(String::new, |target| target.as_str().to_string())
        },
        ToString::to_string,
    );
    baselines.insert(module.key(), baseline);
    Ok(Some(BaselineSpec::explicit(diff_ref)))
}

fn target_for<'a>(targets: &'a ReleaseTargets, module: &Module) -> Option<&'a dyn ReleaseAdapter> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
}

/// The repo-scoped union of the version-reference file globs declared across a
/// project's resolved release settings.
fn version_reference_globs(settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>) -> Vec<String> {
    let mut globs = BTreeSet::new();
    for resolved in settings.values() {
        for reference in &resolved.version_references {
            for glob in &reference.files {
                globs.insert(glob.clone());
            }
        }
    }
    globs.into_iter().collect()
}

/// Whether a repo-relative path matches any of the version-reference globs.
///
/// The change seam renders repo-relative records with a leading `./` (the
/// single-repo member prefix), so the rendering is normalized before matching a
/// glob authored without that prefix (e.g. `README.md`, `crates/*/README.md`).
fn path_matches_any(globs: &[String], path: &std::path::Path) -> bool {
    let rendered = path.to_string_lossy().replace('\\', "/");
    let normalized = rendered.strip_prefix("./").unwrap_or(&rendered);
    globs
        .iter()
        .any(|glob| rskit_util::glob::glob_match(glob, normalized))
}
