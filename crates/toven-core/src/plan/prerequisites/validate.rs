//! Configure-time checks for task `depends_on` and `outputs`.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{ModuleKey, RepoPath};
use toven_ports::{DependencyScope, TaskDependency, TaskEntry};

use super::expand::forward_edges;
use crate::plan::configure::MemberAdapters;
use crate::plan::discover::Federation;
use crate::plan::overrides::GroupOverrides;

/// Validate every configured task table's prerequisites and outputs.
///
/// Within one ecosystem's table: each `depends_on` names a task in the same
/// table, no prerequisite is persistent (it never finishes), and the task
/// graph has no cycle. `"^build"` on `build` itself is not a cycle: it walks
/// down the module graph. Each `outputs` entry is a path inside the module.
///
/// # Errors
/// [`AppError::invalid_input`] naming the offending `ecosystems.<id>.tasks.<name>`
/// field.
pub(in crate::plan) fn validate(adapters: &MemberAdapters) -> AppResult<()> {
    for (_, ecosystem, adapter) in adapters.iter() {
        let ecosystem = ecosystem.as_str();
        let tasks = &adapter.common().tasks;
        for (name, entry) in tasks {
            let field = format!("ecosystems.{ecosystem}.tasks.{name}.depends_on");
            check_prerequisites(ecosystem, &field, name, &entry.depends_on, tasks)?;
            check_outputs(
                &format!("ecosystems.{ecosystem}.tasks.{name}.outputs"),
                name,
                &entry.outputs,
            )?;
        }
        let table = tasks
            .iter()
            .map(|(name, entry)| (name.as_str(), entry.depends_on.as_slice()))
            .collect();
        check_cycles(&table, |first| {
            format!("ecosystems.{ecosystem}.tasks.{first}.depends_on")
        })?;
    }
    Ok(())
}

/// Validate every group task override's `depends_on`, `persistent` and
/// `outputs` against its members' ecosystem task tables.
///
/// A replaced `depends_on` must name known tasks and keep each member's
/// effective table acyclic, outputs must stay inside the module, and no
/// prerequisite may land on a unit that is persistent once the overrides
/// apply (see [`check_persistence`]).
///
/// # Errors
/// [`AppError::invalid_input`] naming the offending
/// `groups.<name>.tasks.<task>.<field>` (or ecosystem) field.
pub(in crate::plan) fn validate_overrides(
    adapters: &MemberAdapters,
    federation: &Federation,
    overrides: &GroupOverrides,
) -> AppResult<()> {
    let mut replaced: Replaced<'_> = BTreeMap::new();
    for (module, task, identity, over) in overrides.tasks() {
        if let Some(outputs) = &over.outputs {
            check_outputs(
                &format!("groups.{identity}.tasks.{task}.outputs"),
                task,
                outputs,
            )?;
        }
        if let Some(depends_on) = over.depends_on.as_deref() {
            replaced
                .entry(module)
                .or_default()
                .insert(task, (identity, depends_on));
        }
    }
    for (module, replaced) in &replaced {
        let Some(tasks) = task_table(adapters, module) else {
            continue;
        };
        let ecosystem = module.module.ecosystem.as_str();
        for (task, (identity, depends_on)) in replaced {
            let field = format!("groups.{identity}.tasks.{task}.depends_on");
            for dependency in *depends_on {
                check_known(ecosystem, &field, task, dependency, tasks)?;
            }
        }
        let table = tasks
            .iter()
            .map(|(name, entry)| {
                let depends_on = replaced
                    .get(name.as_str())
                    .map_or(entry.depends_on.as_slice(), |(_, depends_on)| *depends_on);
                (name.as_str(), depends_on)
            })
            .collect();
        check_cycles(&table, |first| match replaced.get(first) {
            Some((identity, _)) => format!("groups.{identity}.tasks.{first}.depends_on"),
            None => format!("ecosystems.{ecosystem}.tasks.{first}.depends_on"),
        })?;
    }
    check_persistence(adapters, federation, overrides, &replaced)
}

/// Per module, the tasks whose `depends_on` a group replaces, with the group
/// identity and the replacement list.
type Replaced<'a> = BTreeMap<&'a ModuleKey, BTreeMap<&'a str, (&'a str, &'a [TaskDependency])>>;

/// Reject a prerequisite that lands on a persistent unit once group overrides
/// apply: the dependent would never start.
///
/// Persistence belongs to the module a prerequisite lands on: `"build"` lands
/// on the same module and `"^build"` on each module it depends on, and either
/// may carry a group `persistent` override. The error names the group field
/// that introduced the problem: the replaced `depends_on` when there is one,
/// else the `persistent` override.
fn check_persistence(
    adapters: &MemberAdapters,
    federation: &Federation,
    overrides: &GroupOverrides,
    replaced: &Replaced<'_>,
) -> AppResult<()> {
    let persistent_override = overrides
        .tasks()
        .any(|(_, _, _, over)| over.persistent == Some(true));
    if replaced.is_empty() && !persistent_override {
        return Ok(());
    }
    let dependencies = forward_edges(federation);
    for module in &federation.modules {
        let module = module.key();
        let Some(tasks) = task_table(adapters, &module) else {
            continue;
        };
        for (name, entry) in tasks {
            let replacement = replaced
                .get(&module)
                .and_then(|tasks| tasks.get(name.as_str()));
            let depends_on = replacement.map_or(entry.depends_on.as_slice(), |(_, list)| *list);
            for dependency in depends_on {
                let task = dependency.task();
                let targets: Vec<&ModuleKey> = match dependency.scope() {
                    DependencyScope::Module => vec![&module],
                    DependencyScope::Dependencies => {
                        dependencies.get(&module).into_iter().flatten().collect()
                    }
                };
                for target in targets {
                    let Some(base) = task_table(adapters, target).and_then(|tasks| tasks.get(task))
                    else {
                        continue;
                    };
                    let made = overrides.task(target, task).and_then(|(identity, over)| {
                        over.persistent.map(|value| (identity, value))
                    });
                    if !made.map_or(base.persistent, |(_, value)| value) {
                        continue;
                    }
                    let field = match (replacement, made) {
                        (Some((identity, _)), _) => {
                            format!("groups.{identity}.tasks.{name}.depends_on")
                        }
                        (None, Some((identity, _))) => {
                            format!("groups.{identity}.tasks.{task}.persistent")
                        }
                        (None, None) => format!(
                            "ecosystems.{}.tasks.{name}.depends_on",
                            module.module.ecosystem
                        ),
                    };
                    return Err(AppError::invalid_input(
                        field,
                        format!(
                            "task '{name}' on '{module}' depends on '{dependency}', but '{task}' is persistent on '{target}' and never finishes. Drop the persistent override or that dependency."
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The configured task table of the ecosystem owning `module`.
fn task_table<'a>(
    adapters: &'a MemberAdapters,
    module: &ModuleKey,
) -> Option<&'a BTreeMap<String, TaskEntry>> {
    adapters
        .get(module.member.as_ref(), &module.module.ecosystem)
        .map(|adapter| &adapter.common().tasks)
}

fn check_prerequisites(
    ecosystem: &str,
    field: &str,
    name: &str,
    depends_on: &[TaskDependency],
    tasks: &BTreeMap<String, TaskEntry>,
) -> AppResult<()> {
    for dependency in depends_on {
        if check_known(ecosystem, field, name, dependency, tasks)?.persistent {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "task '{name}' depends on '{dependency}', but '{}' is persistent and never finishes. Remove it from `depends_on`.",
                    dependency.task()
                ),
            ));
        }
    }
    Ok(())
}

/// The table entry `dependency` names, or an error listing the known tasks.
fn check_known<'a>(
    ecosystem: &str,
    field: &str,
    name: &str,
    dependency: &TaskDependency,
    tasks: &'a BTreeMap<String, TaskEntry>,
) -> AppResult<&'a TaskEntry> {
    tasks.get(dependency.task()).ok_or_else(|| {
        let known = tasks
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        AppError::invalid_input(
            field,
            format!(
                "task '{name}' depends on '{dependency}', but ecosystem '{ecosystem}' has no '{}' task. Known tasks: {known}.",
                dependency.task()
            ),
        )
    })
}

fn check_outputs(field: &str, name: &str, outputs: &[String]) -> AppResult<()> {
    for output in outputs {
        let inside =
            RepoPath::new(output).is_ok_and(|path| path.as_path() != std::path::Path::new("."));
        if !inside {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "output '{output}' of task '{name}' must be a path inside the module, such as 'dist'"
                ),
            ));
        }
    }
    Ok(())
}

/// Reject a cycle among task prerequisites, ignoring `^self` (which recurses
/// down the module graph, not back to the same unit). `field` names the
/// config field of the cycle's first task.
fn check_cycles(
    table: &BTreeMap<&str, &[TaskDependency]>,
    field: impl Fn(&str) -> String,
) -> AppResult<()> {
    let edges: BTreeMap<&str, BTreeSet<&str>> = table
        .iter()
        .map(|(name, depends_on)| {
            let targets = depends_on
                .iter()
                .filter(|dependency| {
                    !(dependency.scope() == DependencyScope::Dependencies
                        && dependency.task() == *name)
                })
                .map(TaskDependency::task)
                .collect();
            (*name, targets)
        })
        .collect();

    let mut done = BTreeSet::new();
    for start in edges.keys() {
        let mut path = Vec::new();
        if let Some(cycle) = find_cycle(start, &edges, &mut path, &mut done) {
            let first = cycle.first().copied().unwrap_or_default();
            return Err(AppError::invalid_input(
                field(first),
                format!(
                    "task prerequisites form a cycle: {}. Remove one of these `depends_on` entries.",
                    cycle.join(" -> ")
                ),
            ));
        }
    }
    Ok(())
}

/// Depth-first search that returns the first cycle reachable from `node`.
fn find_cycle<'a>(
    node: &'a str,
    edges: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    path: &mut Vec<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Option<Vec<&'a str>> {
    if let Some(position) = path.iter().position(|seen| *seen == node) {
        let mut cycle = path.get(position..).unwrap_or_default().to_vec();
        cycle.push(node);
        return Some(cycle);
    }
    if done.contains(node) {
        return None;
    }
    path.push(node);
    for next in edges.get(node).into_iter().flatten() {
        if let Some(cycle) = find_cycle(next, edges, path, done) {
            return Some(cycle);
        }
    }
    path.pop();
    done.insert(node);
    None
}
