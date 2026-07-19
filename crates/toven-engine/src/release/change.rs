//! Release change detection.
//!
//! Detection is member-scoped: each member repo contributes its own worktree
//! status, tag namespace, and configured baseline, while the changed seeds are
//! still resolved against the one federated umbrella graph. A single-repo
//! project is the N=1 degenerate member (no id, empty path prefix).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::AppResult;
use toven_model::{MemberId, Module, ModuleKey};
use toven_ports::{BaselineSpec, ChangeRecord, ReleaseTarget, TagRef, TagScheme};

use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};
use crate::federation::compose::ComposedMember;
use crate::plan::{PlanContext, Selection};

use super::{ReleaseBaseline, ReleaseTargets, ResolvedReleaseSettings, tag};

/// Per-module change-detection output.
#[derive(Debug, Clone)]
pub(super) struct ReleaseChanges {
    pub(super) changed: BTreeSet<ModuleKey>,
    pub(super) records: BTreeMap<ModuleKey, Vec<ChangeRecord>>,
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
    selection: &Selection,
    base_override: Option<&str>,
    readers: &MemberVcsReaders<'_>,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<ReleaseChanges> {
    let mut changes = ReleaseChanges {
        changed: BTreeSet::new(),
        records: BTreeMap::new(),
        baselines: BTreeMap::new(),
    };
    for reader in readers.entries() {
        detect_member(
            context,
            selection,
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
    selection: &Selection,
    base_override: Option<&str>,
    reader: &MemberVcsReader<'_>,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    changes: &mut ReleaseChanges,
) -> AppResult<()> {
    let member = reader.member();
    let base_ref = member_base_ref(context, member);
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
            base_ref,
            base_override,
            selection,
            &tags,
            &scheme,
            &mut changes.baselines,
        ) else {
            changes.changed.insert(module.key());
            changes.records.insert(module.key(), Vec::new());
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
        }
    }

    Ok(())
}

/// The configured release baseline ref for `member`, from the composed
/// federation.
fn member_base_ref<'a>(context: &'a PlanContext, member: Option<&MemberId>) -> Option<&'a str> {
    context
        .composed
        .members()
        .iter()
        .find(|composed| composed.member().id() == member)
        .and_then(ComposedMember::base_ref)
}

fn baseline_spec(
    module: &Module,
    base_ref: Option<&str>,
    base_override: Option<&str>,
    selection: &Selection,
    tags: &[TagRef],
    scheme: &TagScheme,
    baselines: &mut BTreeMap<ModuleKey, ReleaseBaseline>,
) -> Option<BaselineSpec> {
    // Record the anchoring baseline (a release tag is preferred, since it also
    // carries the version that offline idempotency anchors on).
    let tag_spec = tag::latest(scheme, tags).map(|(version, release_tag)| {
        baselines.insert(
            module.key(),
            ReleaseBaseline::tag(
                module.key(),
                release_tag.name.clone(),
                version,
                release_tag.target.clone(),
            ),
        );
        BaselineSpec::explicit(release_tag.target.as_str().to_string())
    });

    // `--base` overrides the diff ref (default: the latest release tag) while the
    // tag still anchors idempotency.
    if let Some(base) = base_override {
        let spec = BaselineSpec::explicit(base.to_string());
        baselines
            .entry(module.key())
            .or_insert_with(|| ReleaseBaseline::fallback(module.key(), spec.clone()));
        return Some(spec);
    }

    if let Some(spec) = tag_spec {
        return Some(spec);
    }

    if let Selection::Changed(Some(spec)) = selection {
        baselines.insert(
            module.key(),
            ReleaseBaseline::fallback(module.key(), spec.clone()),
        );
        return Some(spec.clone());
    }

    if let Some(base_ref) = base_ref {
        let spec = BaselineSpec::explicit(base_ref.to_string());
        baselines.insert(
            module.key(),
            ReleaseBaseline::fallback(module.key(), spec.clone()),
        );
        return Some(spec);
    }

    None
}

fn target_for<'a>(targets: &'a ReleaseTargets, module: &Module) -> Option<&'a dyn ReleaseTarget> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
}
