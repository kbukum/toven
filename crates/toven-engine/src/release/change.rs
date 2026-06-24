//! Release change detection.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::AppResult;
use toven_model::{Module, ModuleRef};
use toven_ports::{BaselineSpec, ChangeRecord, VcsReader};

use crate::config::Document;
use crate::plan::{PlanContext, Selection};

use super::{ReleaseBaseline, tag};

/// Per-module change-detection output.
#[derive(Debug, Clone)]
pub(super) struct ReleaseChanges {
    pub(super) changed: BTreeSet<ModuleRef>,
    pub(super) records: BTreeMap<ModuleRef, Vec<ChangeRecord>>,
    pub(super) baselines: BTreeMap<ModuleRef, ReleaseBaseline>,
}

/// Detect modules changed since their release baseline.
pub(super) fn detect(
    context: &PlanContext,
    document: &Document,
    selection: &Selection,
    vcs: &dyn VcsReader,
) -> AppResult<ReleaseChanges> {
    let mut changed = BTreeSet::new();
    let mut records = BTreeMap::new();
    let mut baselines = BTreeMap::new();
    let worktree = vcs.worktree_status()?;

    for module in &context.federation.modules {
        let Some(spec) = baseline_spec(module, document, selection, vcs, &mut baselines)? else {
            changed.insert(module.id.clone());
            records.insert(module.id.clone(), Vec::new());
            continue;
        };
        let mut module_changes = vcs.changed_since(&spec)?;
        module_changes.extend(worktree.clone());
        let seeds =
            crate::plan::changed_seeds(&module_changes, &context.graph, &context.federation);
        if seeds.contains(&module.id) {
            changed.insert(module.id.clone());
            records.insert(module.id.clone(), module_changes);
        }
    }

    Ok(ReleaseChanges {
        changed,
        records,
        baselines,
    })
}

fn baseline_spec(
    module: &Module,
    document: &Document,
    selection: &Selection,
    vcs: &dyn VcsReader,
    baselines: &mut BTreeMap<ModuleRef, ReleaseBaseline>,
) -> AppResult<Option<BaselineSpec>> {
    let tags = vcs.list_tags(Some(&tag::glob(&module.id)))?;
    if let Some((_version, release_tag)) = tag::latest(&module.id, &tags) {
        baselines.insert(
            module.id.clone(),
            ReleaseBaseline::tag(
                module.id.clone(),
                release_tag.name.clone(),
                release_tag.target.clone(),
            ),
        );
        return Ok(Some(BaselineSpec::explicit(
            release_tag.target.as_str().to_string(),
        )));
    }

    if let Selection::Changed(spec) = selection {
        baselines.insert(
            module.id.clone(),
            ReleaseBaseline::fallback(module.id.clone(), spec.clone()),
        );
        return Ok(Some(spec.clone()));
    }

    if let Some(base_ref) = &document.project.base_ref {
        let spec = BaselineSpec::explicit(base_ref.clone());
        baselines.insert(
            module.id.clone(),
            ReleaseBaseline::fallback(module.id.clone(), spec.clone()),
        );
        return Ok(Some(spec));
    }

    Ok(None)
}
