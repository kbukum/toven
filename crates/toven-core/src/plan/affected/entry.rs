//! The selection dispatcher: pick the active module set for a plan request.

use std::collections::BTreeSet;

use rskit_errors::AppResult;
use toven_model::{DepKind, Graph, Module, ModuleKey};
use toven_ports::{ChangeRecord, ChangeStatus, TaskIntent, TaskKind};

use crate::federation::baseline::MemberVcsReaders;

use crate::plan::configure::MemberAdapters;
use crate::plan::discover::Federation;
use crate::plan::request::{PlanRequest, Selection};

use super::changed::changed_for_members;
use super::select::explicit_seeds;
use crate::plan::ownership::{AttributionPolicy, changed_seeds, unclassified_paths};

/// The resolved active module set plus any forced-full-activation diagnostic.
///
/// `modules` is the input to scheduling. `full_activation` is empty for an
/// explicit or all-modules selection; for a changed selection it names the
/// changed paths that no module or workspace could claim, which is exactly why
/// every module was activated (fail-open). The CLI renders it so a full run
/// is never silent; the engine only returns the typed data.
#[derive(Debug, Default)]
#[allow(clippy::redundant_pub_crate)]
pub struct ActiveModules {
    /// The active module keys scheduling operates over.
    pub modules: BTreeSet<ModuleKey>,
    /// Changed paths that forced full activation (empty when not forced).
    pub(crate) full_activation: Vec<String>,
}

/// Resolve the active module set for this request.
///
/// [`Selection::All`] activates every module; [`Selection::Explicit`] resolves
/// the user-named selectors and optionally unions the forward-dependencies
/// and/or reverse-dependents closures; [`Selection::Changed`] maps the changed
/// paths (committed ∪ worktree) reported by every member reader to seed modules
/// and returns the reverse-dependents closure, failing open to the full set
/// (the [`FailOpen`](AttributionPolicy) task rule) on any unclassifiable path.
/// Each member reader uses its own resolved
/// baseline; the single-repo project is the N=1 degenerate member.
///
/// # Errors
/// Propagates [`VcsReader`](toven_ports::VcsReader) failures, selector
/// resolution errors (unknown or ambiguous targets), and the graph closure (an
/// unknown seed).
#[allow(clippy::redundant_pub_crate)]
pub fn active_modules(
    request: &PlanRequest,
    graph: &Graph,
    federation: &Federation,
    vcs: &MemberVcsReaders<'_>,
) -> AppResult<ActiveModules> {
    match &request.selection {
        Selection::All => Ok(ActiveModules::from_set(all_modules(graph))),
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
            Ok(ActiveModules::from_set(active))
        }
        Selection::Changed(spec) => {
            let changed = changed_for_members(vcs, spec.as_ref())?;
            resolve_changed(request, graph, federation, &changed)
        }
        Selection::ChangedPaths(paths) => {
            let changed: Vec<ChangeRecord> = paths
                .iter()
                .map(|path| ChangeRecord::new(path, ChangeStatus::Modified))
                .collect();
            resolve_changed(request, graph, federation, &changed)
        }
    }
}

impl ActiveModules {
    /// Wrap a resolved set with no forced-activation diagnostic.
    const fn from_set(modules: BTreeSet<ModuleKey>) -> Self {
        Self {
            modules,
            full_activation: Vec::new(),
        }
    }
}

/// Resolve a changed-path selection to the active set and its full-activation
/// diagnostic.
///
/// When every changed path is attributable the active set is the
/// reverse-dependents closure of the seeds; an unattributable path fails open
/// (the [`FailOpen`](AttributionPolicy::FailOpen) rule for task selection) to
/// every module and names the offending path(s) so the CLI can explain the full
/// run.
fn resolve_changed(
    request: &PlanRequest,
    graph: &Graph,
    federation: &Federation,
    changed: &[ChangeRecord],
) -> AppResult<ActiveModules> {
    let full_activation = unclassified_paths(changed, federation);
    let seeds = changed_seeds(changed, graph, federation, AttributionPolicy::FailOpen);
    let modules = graph.closure(&seeds, dependents_filter(&request.intent))?;
    Ok(ActiveModules {
        modules,
        full_activation,
    })
}

/// Every module key in the graph.
pub(super) fn all_modules(graph: &Graph) -> BTreeSet<ModuleKey> {
    graph.modules().map(Module::key).collect()
}

/// Narrow an active set to the modules whose ecosystem defines the addressed
/// task.
///
/// A task need not exist in every ecosystem: `structure` may live only in the
/// `command` ecosystem while `vuln` lives only in `rust`. For a broad
/// (all-modules or changed) selection this keeps just the modules whose
/// ecosystem's authoritative config task table declares `intent.name()`, so the
/// task runs wherever it is defined instead of failing every active ecosystem
/// that happens to lack it.
///
/// Two behaviours are deliberately preserved:
/// - An [`Explicit`](Selection::Explicit) selection is returned unchanged — a
///   user who names a target that lacks the task should see the typed
///   per-ecosystem "has no `<task>` task" error, not a silently empty plan.
/// - When *no* active ecosystem defines the task the set is returned unchanged,
///   so scheduling raises that same unknown-task error (and the CLI its typo
///   hint) exactly as before.
///
/// When the set *is* narrowed, the `full_activation`
/// diagnostic is cleared: the plan no longer activates every module, so carrying
/// the forced-full-activation paths would make the CLI emit a misleading
/// `FullActivation` event. The diagnostic is kept only when nothing is dropped.
#[allow(clippy::redundant_pub_crate)]
pub fn restrict_to_task_defining(
    active: ActiveModules,
    request: &PlanRequest,
    adapters: &MemberAdapters,
) -> ActiveModules {
    if matches!(request.selection, Selection::Explicit { .. }) {
        return active;
    }
    let wanted = request.intent.name();
    let defining: BTreeSet<ModuleKey> = active
        .modules
        .iter()
        .filter(|key| ecosystem_defines_task(adapters, key, wanted))
        .cloned()
        .collect();
    // No active ecosystem defines the task, or every one does: nothing is
    // narrowed, so the set and its full-activation diagnostic stay untouched.
    if defining.is_empty() || defining.len() == active.modules.len() {
        return active;
    }
    ActiveModules {
        modules: defining,
        full_activation: Vec::new(),
    }
}

/// Whether the configured adapter that owns `key` declares a `task` entry in its
/// authoritative config task table.
fn ecosystem_defines_task(adapters: &MemberAdapters, key: &ModuleKey, task: &str) -> bool {
    adapters
        .get(key.member(), &key.module().ecosystem)
        .is_some_and(|adapter| adapter.common().tasks.contains_key(task))
}

/// The reverse-dependents edge filter for `intent`.
///
/// Build/normal/overlay edges always propagate; `Dev` edges propagate only for
/// a [`TaskKind::Test`] run (a dev-only change affects tests but not downstream
/// builds).
fn dependents_filter(intent: &TaskIntent) -> impl Fn(DepKind) -> bool {
    let is_test = intent.kind() == TaskKind::Test;
    move |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    }
}

/// The forward-dependencies edge filter for `intent`.
///
/// `--dependencies` expands a selection to everything it needs to build.
/// Build/normal/overlay edges always propagate; `Dev` edges propagate only for
/// a [`TaskKind::Test`] run, mirroring [`dependents_filter`] — a dev-only
/// prerequisite is required to test a module but not to build it.
fn dependencies_filter(intent: &TaskIntent) -> impl Fn(DepKind) -> bool {
    let is_test = intent.kind() == TaskKind::Test;
    move |kind: DepKind| {
        matches!(kind, DepKind::Normal | DepKind::Build | DepKind::Overlay)
            || (is_test && kind == DepKind::Dev)
    }
}
