//! The selection dispatcher: pick the active module set for a plan request.

use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult};
use toven_model::{DepKind, Graph, Module, ModuleKey};
use toven_ports::{ChangeRecord, ChangeStatus, TaskIntent, TaskKind};

use crate::federation::baseline::MemberVcsReaders;

use crate::plan::configure::MemberAdapters;
use crate::plan::discover::Federation;
use crate::plan::graph::GroupIndex;
use crate::plan::request::{PlanRequest, Selection};
use crate::plan::schedule::unknown_task_error;

use super::changed::changed_for_members;
use super::select::{ExplicitSeeds, explicit_seeds};
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
    /// Modules an explicit selection named exactly; scheduling rejects one
    /// that is not eligible for the task instead of skipping it.
    pub(crate) named: BTreeSet<ModuleKey>,
}

/// Resolve the active module set for this request.
///
/// [`Selection::All`] activates every module; [`Selection::Explicit`] resolves
/// the user-named selectors and optionally unions the forward-dependencies
/// and/or reverse-dependents closures; [`Selection::Changed`] maps the changed
/// paths (committed ∪ worktree) reported by every member reader to seed modules
/// and returns the reverse-dependents closure, failing open to the full set
/// (the [`FailOpen`](AttributionPolicy) task rule) on any unclassifiable path.
/// Modules with incomplete mutable inputs also seed changed selections, since
/// repository diffs cannot prove those dependencies unchanged.
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
    groups: &GroupIndex,
    vcs: &MemberVcsReaders<'_>,
) -> AppResult<ActiveModules> {
    match &request.selection {
        Selection::All => Ok(ActiveModules::from_set(all_modules(graph))),
        Selection::Explicit {
            targets,
            include_dependents,
            include_dependencies,
        } => {
            let ExplicitSeeds { seeds, named } =
                explicit_seeds(targets, graph, federation, groups)?;
            let mut active = seeds.clone();
            if *include_dependencies {
                active.extend(
                    graph.dependencies_closure(&seeds, dependencies_filter(&request.intent))?,
                );
            }
            if *include_dependents {
                active.extend(graph.closure(&seeds, dependents_filter(&request.intent))?);
            }
            Ok(ActiveModules {
                modules: active,
                full_activation: Vec::new(),
                named,
            })
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
            named: BTreeSet::new(),
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
    let mut seeds = changed_seeds(changed, graph, federation, AttributionPolicy::FailOpen);
    seeds.extend(
        federation
            .modules
            .iter()
            // Version control cannot see an incomplete or external input, so
            // those modules always count as changed; the cache keeps reruns cheap.
            .filter(|module| !module.cache_inputs_complete || !module.external_roots.is_empty())
            .map(Module::key),
    );
    let modules = graph.closure(&seeds, dependents_filter(&request.intent))?;
    Ok(ActiveModules {
        modules,
        full_activation,
        named: BTreeSet::new(),
    })
}

/// Every module key in the graph.
pub(super) fn all_modules(graph: &Graph) -> BTreeSet<ModuleKey> {
    graph.modules().map(Module::key).collect()
}

/// How an [`Explicit`](Selection::Explicit) selection meets a task that some
/// selected modules do not run.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExplicitScope {
    /// The only requested task: a selected module without it is an error.
    Strict,
    /// One task of a list or set: it runs on the selected modules it applies to.
    Narrow,
}

/// Narrow an active set to the modules whose ecosystem defines the addressed
/// task.
///
/// A task need not exist in every ecosystem: `structure` may live only in the
/// `command` ecosystem while `vuln` lives only in `rust`. For a broad
/// (all-modules or changed) selection this keeps just the modules whose
/// ecosystem's authoritative config task table declares `intent.name()`, so the
/// task runs wherever it is defined instead of failing every active ecosystem
/// that happens to lack it. Whether a defining module is *eligible* (the task's
/// `modules` list and `requires`) is decided at scheduling, which reports the
/// skips.
///
/// Two behaviours are deliberately preserved:
/// - Under [`ExplicitScope::Strict`] (a single task) an
///   [`Explicit`](Selection::Explicit) selection is returned unchanged — a
///   user who names a target that lacks the task, or that the task cannot run
///   on, should see a typed scheduling error, not a silently empty plan.
///   Under [`ExplicitScope::Narrow`] (one task of a list or set) the explicit
///   selection narrows like a broad one and stops counting as named, so
///   `build,lint -m worker` runs the build and reports `lint` as skipped on
///   `worker`; the caller rejects a list that plans nothing on the selection.
/// - When *no* active ecosystem defines the task the set is returned unchanged,
///   so scheduling raises that same unknown-task error (and the CLI its typo
///   hint) exactly as before. Under [`ExplicitScope::Narrow`] this applies only
///   when no ecosystem at all defines it; otherwise the task is skipped.
///
/// The `full_activation` diagnostic survives narrowing: a forced full run still
/// activates every module that defines the task, and the CLI must say why.
///
/// # Errors
/// A `modules` entry on the requested task that names no discovered module of
/// its ecosystem.
#[allow(clippy::redundant_pub_crate)]
pub fn restrict_to_task_defining(
    active: ActiveModules,
    request: &PlanRequest,
    federation: &Federation,
    adapters: &MemberAdapters,
    explicit: ExplicitScope,
) -> AppResult<ActiveModules> {
    let wanted = request.intent.name();
    check_task_modules(federation, adapters, wanted)?;
    if explicit == ExplicitScope::Strict && matches!(request.selection, Selection::Explicit { .. })
    {
        return Ok(active);
    }
    let defining: BTreeSet<ModuleKey> = active
        .modules
        .iter()
        .filter(|key| task_entry(adapters, key, wanted).is_some())
        .cloned()
        .collect();
    // No active ecosystem defines the task. In a list, a task some other
    // ecosystem defines simply has nothing to run on the selected modules;
    // otherwise the task is unknown across the whole selection.
    if defining.is_empty() && !active.modules.is_empty() {
        let defined_elsewhere = explicit == ExplicitScope::Narrow
            && adapters
                .iter()
                .any(|(_, _, adapter)| adapter.common().tasks.contains_key(wanted));
        if !defined_elsewhere {
            return Err(unknown_in_selection(&active.modules, adapters, request));
        }
    }
    Ok(ActiveModules {
        modules: defining,
        full_activation: active.full_activation,
        // A list never fails on one task's ineligible module; it reports the skip.
        named: match explicit {
            ExplicitScope::Strict => active.named,
            ExplicitScope::Narrow => BTreeSet::new(),
        },
    })
}

/// The unknown-task error for a selection where no module's ecosystem defines
/// the task: it names every selected ecosystem and suggests from all of their
/// task names.
fn unknown_in_selection(
    modules: &BTreeSet<ModuleKey>,
    adapters: &MemberAdapters,
    request: &PlanRequest,
) -> AppError {
    let mut ecosystems = BTreeSet::new();
    let mut names = BTreeSet::new();
    for key in modules {
        let ecosystem = &key.module().ecosystem;
        ecosystems.insert(ecosystem.as_str());
        if let Some(adapter) = adapters.get(key.member(), ecosystem) {
            names.extend(adapter.common().tasks.keys().map(String::as_str));
        }
    }
    let ecosystems: Vec<&str> = ecosystems.into_iter().collect();
    unknown_task_error(&ecosystems, &request.intent, names)
}

/// The config entry for `task` in the adapter that owns `key`, if it declares one.
fn task_entry<'a>(
    adapters: &'a MemberAdapters,
    key: &ModuleKey,
    task: &str,
) -> Option<&'a toven_ports::TaskEntry> {
    adapters
        .get(key.member(), &key.module().ecosystem)
        .and_then(|adapter| adapter.common().tasks.get(task))
}

/// Reject a `modules` list on `task` that names a module its ecosystem did not
/// discover, so a typo never silently disables the task.
fn check_task_modules(
    federation: &Federation,
    adapters: &MemberAdapters,
    task: &str,
) -> AppResult<()> {
    for (member, ecosystem, adapter) in adapters.iter() {
        let Some(entry) = adapter.common().tasks.get(task) else {
            continue;
        };
        for name in &entry.modules {
            let known = federation.modules.iter().any(|module| {
                module.member.as_ref() == member
                    && module.id.ecosystem == *ecosystem
                    && module.id.name == *name
            });
            if !known {
                return Err(AppError::invalid_input(
                    format!("ecosystems.{ecosystem}.tasks.{task}.modules"),
                    format!("'{name}' is not a discovered {ecosystem} module"),
                ));
            }
        }
    }
    Ok(())
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
