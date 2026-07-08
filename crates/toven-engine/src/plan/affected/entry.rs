//! The selection dispatcher: pick the active module set for a plan request.

use std::collections::BTreeSet;

use rskit_errors::AppResult;
use toven_model::{DepKind, Graph, Module, ModuleKey};
use toven_ports::{ChangeRecord, ChangeStatus, TaskKind};

use crate::federation::baseline::MemberVcsReaders;

use crate::plan::discover::Federation;
use crate::plan::request::{PlanRequest, Selection};

use super::changed::{changed_for_members, changed_seeds};
use super::select::explicit_seeds;

/// Resolve the active module set for this request.
///
/// [`Selection::All`] activates every module; [`Selection::Explicit`] resolves
/// the user-named selectors and optionally unions the forward-dependencies and/or
/// reverse-dependents closures; [`Selection::Changed`] maps the changed paths
/// (committed ∪ worktree) reported by every member reader to seed modules and
/// returns the reverse-dependents closure, failing closed to the full set on any
/// unclassifiable path. Each member reader uses its own resolved baseline; the
/// single-repo project is the N=1 degenerate member.
///
/// # Errors
/// Propagates [`VcsReader`](toven_ports::VcsReader) failures, selector resolution
/// errors (unknown or ambiguous targets), and the graph closure (an unknown seed).
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn active_modules(
    request: &PlanRequest,
    graph: &Graph,
    federation: &Federation,
    vcs: &MemberVcsReaders<'_>,
) -> AppResult<BTreeSet<ModuleKey>> {
    match &request.selection {
        Selection::All => Ok(all_modules(graph)),
        Selection::Explicit {
            targets,
            include_dependents,
            include_dependencies,
        } => {
            let seeds = explicit_seeds(targets, graph, federation)?;
            let mut active = seeds.clone();
            if *include_dependencies {
                active.extend(
                    graph.dependencies_closure(&seeds, dependencies_filter(&request.intent))?,
                );
            }
            if *include_dependents {
                active.extend(graph.closure(&seeds, dependents_filter(&request.intent))?);
            }
            Ok(active)
        }
        Selection::Changed(spec) => {
            let changed = changed_for_members(vcs, spec.as_ref())?;
            let seeds = changed_seeds(&changed, graph, federation);
            graph.closure(&seeds, dependents_filter(&request.intent))
        }
        Selection::ChangedPaths(paths) => {
            let changed: Vec<ChangeRecord> = paths
                .iter()
                .map(|path| ChangeRecord::new(path, ChangeStatus::Modified))
                .collect();
            let seeds = changed_seeds(&changed, graph, federation);
            graph.closure(&seeds, dependents_filter(&request.intent))
        }
    }
}

/// Every module key in the graph.
pub(super) fn all_modules(graph: &Graph) -> BTreeSet<ModuleKey> {
    graph.modules().map(Module::key).collect()
}

/// The reverse-dependents edge filter for `intent`.
///
/// Build/normal/overlay edges always propagate; `Dev` edges propagate only for a
/// [`TaskKind::Test`] run (a dev-only change affects tests but not downstream
/// builds).
fn dependents_filter(intent: &TaskKind) -> impl Fn(DepKind) -> bool {
    let is_test = matches!(intent, TaskKind::Test);
    move |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    }
}

/// The forward-dependencies edge filter for `intent`.
///
/// `--dependencies` expands a selection to everything it needs to build.
/// Build/normal/overlay edges always propagate; `Dev` edges propagate only for a
/// [`TaskKind::Test`] run, mirroring [`dependents_filter`] — a dev-only
/// prerequisite is required to test a module but not to build it.
fn dependencies_filter(intent: &TaskKind) -> impl Fn(DepKind) -> bool {
    let is_test = matches!(intent, TaskKind::Test);
    move |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    }
}
