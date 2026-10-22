//! Expand requested tasks through their `depends_on` into prerequisite
//! (task, module) pairs.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rskit_errors::AppResult;
use toven_model::ModuleKey;
use toven_ports::{DependencyScope, TaskDependency, TaskEntry, TaskIntent};

use crate::plan::affected::ActiveModules;
use crate::plan::configure::MemberAdapters;
use crate::plan::discover::Federation;
use crate::plan::overrides::GroupOverrides;
use crate::plan::request::PlanRequest;
use crate::plan::schedule;

/// One task on one module: a node of the prerequisite graph.
pub(in crate::plan) type Node = (String, ModuleKey);

/// Each node's direct prerequisite nodes. Empty when no planned task declares
/// `depends_on`.
pub(in crate::plan) type Needs = BTreeMap<Node, BTreeSet<Node>>;

/// The per-task requests and active sets to schedule, with the prerequisite
/// graph that links their units afterwards.
pub(in crate::plan) struct Expanded {
    /// One request per task: the requested ones, then any prerequisite-only
    /// task in name order.
    pub(in crate::plan) requests: Vec<PlanRequest>,
    /// Each request's active modules, widened by the prerequisite modules.
    pub(in crate::plan) actives: Vec<ActiveModules>,
    /// The prerequisite graph over every planned node.
    pub(in crate::plan) needs: Needs,
}

/// The configuration the expansion reads.
pub(in crate::plan) struct Scope<'a> {
    pub(in crate::plan) federation: &'a Federation,
    pub(in crate::plan) adapters: &'a MemberAdapters,
    pub(in crate::plan) overrides: &'a GroupOverrides,
}

/// Walk each requested task's eligible modules through `depends_on`.
///
/// `"build"` adds `build` on the same module; `"^build"` adds `build` on each
/// module the module depends on (every edge kind), and the walk continues
/// through each added node's own `depends_on`. A module whose ecosystem has no
/// such task contributes nothing. Prerequisite modules join the request for
/// the same task when one exists (so a whole-workspace unit is planned once);
/// otherwise a prerequisite-only request is added, without passthrough args,
/// which belong to the requested task.
///
/// Prerequisite modules are never `named`: one the prerequisite task cannot run
/// on is skipped and its own prerequisites are still honored.
///
/// # Errors
/// Propagates an effective-task resolution failure for a requested task.
pub(in crate::plan) fn expand(
    requests: Vec<PlanRequest>,
    mut actives: Vec<ActiveModules>,
    scope: &Scope<'_>,
) -> AppResult<Expanded> {
    let mut queue = VecDeque::new();
    let mut planned: BTreeSet<Node> = BTreeSet::new();
    for (request, active) in requests.iter().zip(&actives) {
        let name = request.intent.name();
        for module in &active.modules {
            planned.insert((name.to_string(), module.clone()));
        }
        let eligible = schedule::eligible_modules(
            scope.federation,
            &active.modules,
            scope.adapters,
            &request.intent,
            scope.overrides,
        )?;
        queue.extend(
            eligible
                .into_iter()
                .map(|module| (name.to_string(), module)),
        );
    }

    let dependencies = forward_edges(scope.federation);
    let mut needs = Needs::new();
    let mut seen: BTreeSet<Node> = queue.iter().cloned().collect();
    let mut extra: BTreeMap<String, BTreeSet<ModuleKey>> = BTreeMap::new();
    while let Some(node) = queue.pop_front() {
        let Some(entry) = task_entry(scope.adapters, &node) else {
            continue;
        };
        for dependency in effective_depends_on(entry, scope.overrides, &node) {
            let targets: Vec<&ModuleKey> = match dependency.scope() {
                DependencyScope::Module => vec![&node.1],
                DependencyScope::Dependencies => {
                    dependencies.get(&node.1).into_iter().flatten().collect()
                }
            };
            for target in targets {
                let prerequisite = (dependency.task().to_string(), target.clone());
                if task_entry(scope.adapters, &prerequisite).is_none() {
                    continue;
                }
                needs
                    .entry(node.clone())
                    .or_default()
                    .insert(prerequisite.clone());
                if seen.insert(prerequisite.clone()) {
                    if !planned.contains(&prerequisite) {
                        extra
                            .entry(prerequisite.0.clone())
                            .or_default()
                            .insert(prerequisite.1.clone());
                    }
                    queue.push_back(prerequisite);
                }
            }
        }
    }

    let mut requests = requests;
    let base = requests.first().cloned();
    for (task, modules) in extra {
        if let Some(index) = requests
            .iter()
            .position(|request| request.intent.name() == task)
        {
            if let Some(active) = actives.get_mut(index) {
                active.modules.extend(modules);
            }
            continue;
        }
        let Some(base) = &base else { continue };
        let mut request = base.clone().with_passthrough(Vec::new());
        request.intent = TaskIntent::resolve(task);
        requests.push(request);
        actives.push(ActiveModules {
            modules,
            full_activation: Vec::new(),
            named: BTreeSet::new(),
        });
    }
    Ok(Expanded {
        requests,
        actives,
        needs,
    })
}

/// The configured task `node.0` in the ecosystem table owning `node.1`.
fn task_entry<'a>(adapters: &'a MemberAdapters, node: &Node) -> Option<&'a TaskEntry> {
    let (task, module) = node;
    adapters
        .get(module.member.as_ref(), &module.module.ecosystem)
        .and_then(|adapter| adapter.common().tasks.get(task))
}

/// The prerequisites of `node`: a group override's `depends_on` for the
/// module when one is set, else the ecosystem task's.
fn effective_depends_on<'a>(
    entry: &'a TaskEntry,
    overrides: &'a GroupOverrides,
    (task, module): &Node,
) -> &'a [TaskDependency] {
    overrides
        .task(module, task)
        .and_then(|(_, over)| over.depends_on.as_deref())
        .unwrap_or(&entry.depends_on)
}

/// Each module's direct dependencies across every edge kind.
pub(super) fn forward_edges(federation: &Federation) -> BTreeMap<ModuleKey, BTreeSet<ModuleKey>> {
    let mut edges: BTreeMap<ModuleKey, BTreeSet<ModuleKey>> = BTreeMap::new();
    for edge in &federation.edges {
        edges
            .entry(edge.from.clone())
            .or_default()
            .insert(edge.to.clone());
    }
    edges
}
