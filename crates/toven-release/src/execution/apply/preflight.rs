use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{Module, ModuleKey};
use toven_ports::{TagSigner, VcsReader, VcsWriter};

use crate::ReleasePlan;

use super::staging::{module_for, target_for};
use super::tagging::{entry_tag_selected, planned_tag_name, tag_message};

/// Pre-commit target preflight: every planned entry must resolve a release
/// target for its (member, ecosystem) pair. A member without a target fails
/// closed here, before any mutation, instead of being discovered mid-apply.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn preflight_targets(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
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
pub(super) fn planned_tag_annotations(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
) -> AppResult<BTreeMap<String, Option<String>>> {
    let mut planned: BTreeMap<String, Option<String>> = BTreeMap::new();
    for entry in &plan.entries {
        let Some(version) = &entry.planned_version else {
            continue;
        };
        if !entry_tag_selected(entry) {
            continue;
        }
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

/// Preflight every distinct planned signed tag before manifest mutation.
///
/// Modules may collapse onto one shared tag, so signer settings must agree for
/// that tag. The writer then validates local signer requirements (including an
/// inherited `user.signingkey`) before the release crosses the commit boundary.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn preflight_tag_signers(plan: &ReleasePlan, writer: &dyn VcsWriter) -> AppResult<()> {
    let mut planned: BTreeMap<String, Option<TagSigner>> = BTreeMap::new();
    for entry in &plan.entries {
        if entry.planned_version.is_none() {
            continue;
        }
        if !entry_tag_selected(entry) {
            continue;
        }
        let name = planned_tag_name(entry)?;
        let signer = entry.signer.clone();
        if let Some(existing) = planned.get(name) {
            if existing != &signer {
                return Err(AppError::invalid_input(
                    "release.sign_tags",
                    format!(
                        "modules sharing release tag '{name}' disagree on tag signing settings; \
                         give the shared tag one signer or a distinct tag_format"
                    ),
                ));
            }
            continue;
        }
        if let Some(signer) = &signer {
            writer.preflight_tag_signer(signer)?;
        }
        planned.insert(name.to_string(), signer);
    }
    Ok(())
}
