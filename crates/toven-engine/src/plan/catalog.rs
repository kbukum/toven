//! Task catalog: a typed projection of the resolved tasks per ecosystem.
//!
//! Discovery (`toven tasks`) and self-correcting suggestions both need the same
//! answer to "what can I run?" — the fully resolved task set, keyed by ecosystem,
//! carrying the canonical name a user must type. This module derives that
//! projection from the already-configured adapters (materializing the same
//! authoritative config task table that the planner consumes, exposed via
//! [`ConfiguredAdapter::common`](toven_ports::ConfiguredAdapter::common)),
//! returning data only: the CLI renders it, and the
//! scheduler reuses the candidate names for "did you mean?" enrichment. Nothing
//! here prints.

use std::collections::HashSet;

use rskit_errors::AppResult;
use toven_ports::{FanOut, Provider, TaskOrigin};

use super::configure::configure;
use crate::config::Document;

/// The resolved task set of a project, grouped by ecosystem.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TaskCatalog {
    /// Per-ecosystem task groups, in canonical (sorted) ecosystem order.
    pub ecosystems: Vec<EcosystemTasks>,
}

impl TaskCatalog {
    /// Every canonical task name across every ecosystem, de-duplicated in first-
    /// seen order. The candidate set for nearest-match suggestions.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for eco in &self.ecosystems {
            for task in &eco.tasks {
                if seen.insert(task.name.as_str()) {
                    names.push(task.name.clone());
                }
            }
        }
        names
    }
}

/// One ecosystem's resolved tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EcosystemTasks {
    /// The ecosystem id (`rust`, `go`, …).
    pub ecosystem: String,
    /// The resolved tasks, in the adapter's declaration order.
    pub tasks: Vec<TaskSummary>,
}

/// A single resolved task, projected for discovery and suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TaskSummary {
    /// The canonical name a user types after `toven`/`toven run` — the task's
    /// identity (the config table key).
    pub name: String,
    /// The task's recognized kind name (`build`, `test`, …, or `default` for a
    /// task with no recognized kind).
    pub kind: String,
    /// Where this resolved task came from (adapter default / project / group).
    pub origin: TaskOrigin,
    /// The intrinsic fan-out capability.
    pub fan_out: FanOut,
    /// Whether the task is persistent (a long-lived run/watch process).
    pub persistent: bool,
    /// The resolved base argv template (before per-module rendering).
    pub argv: Vec<String>,
    /// Extra workspace-relative cache inputs shared by every module.
    pub shared_inputs: Vec<String>,
}

/// Build the [`TaskCatalog`] for `document` against the compiled-in `providers`.
///
/// Reuses the Configure phase (`configure`) so the projected names match the
/// tasks the planner actually resolves — no duplicate resolution. Ecosystems
/// without a loaded provider are skipped exactly as they are during PLAN.
///
/// # Errors
/// Propagates `configure` failures (provider conflicts, subtree conversion, or a
/// provider's `configure` rejection).
pub fn task_catalog(document: &Document, providers: &[&dyn Provider]) -> AppResult<TaskCatalog> {
    let configured = configure(document, providers)?;
    let mut ecosystems = Vec::with_capacity(configured.len());
    for (ecosystem, adapter) in &configured {
        let tasks = adapter
            .common()
            .tasks
            .iter()
            .map(|(key, entry)| entry.materialize(ecosystem.as_str(), key).map(summarize))
            .collect::<AppResult<Vec<_>>>()?;
        ecosystems.push(EcosystemTasks {
            ecosystem: ecosystem.to_string(),
            tasks,
        });
    }
    Ok(TaskCatalog { ecosystems })
}

/// Project one resolved [`Task`](toven_ports::Task) into a [`TaskSummary`].
fn summarize(task: toven_ports::Task) -> TaskSummary {
    TaskSummary {
        name: task.name,
        kind: task.kind.as_str().to_string(),
        origin: task.origin,
        fan_out: task.fan_out,
        persistent: task.persistent,
        argv: task.argv,
        shared_inputs: task.shared_inputs,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::EcosystemId;
    use toven_ports::{FanOut, Provider, Task, TaskOrigin};
    use toven_testkit::{FakeConfiguredAdapter, FakeProvider};

    use super::task_catalog;
    use crate::config::{Document, ProjectConfig, TovenConfig};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn document_with(ecosystems: &[&str]) -> Document {
        let mut sections = BTreeMap::new();
        for id in ecosystems {
            sections.insert(eid(id), rskit_config::RawValue::Null);
        }
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: sections,
            members: Vec::new(),
        }
    }

    fn provider_with_tasks(id: &str, tasks: Vec<Task>) -> FakeProvider {
        let adapter = FakeConfiguredAdapter::new(eid(id)).with_tasks(tasks);
        FakeProvider::new(eid(id)).with_adapter(adapter)
    }

    #[test]
    fn projects_builtin_named_and_custom_tasks_with_origin() {
        // A whole-workspace format default (canonical `format`, not `fmt`), a
        // project override, and a custom task exercise the name/origin mapping.
        let format = Task::new(
            "format",
            vec!["cargo".into(), "fmt".into()],
            FanOut::WholeWorkspace,
        );
        let mut lint = Task::new(
            "lint",
            vec!["cargo".into(), "clippy".into()],
            FanOut::Batchable,
        );
        lint.origin = TaskOrigin::Project;
        let mut custom = Task::new(
            "bench",
            vec!["cargo".into(), "bench".into()],
            FanOut::PerModule,
        );
        custom.origin = TaskOrigin::Project;

        let provider = provider_with_tasks("rust", vec![format, lint, custom]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let catalog = task_catalog(&document_with(&["rust"]), &providers).expect("catalog");

        assert_eq!(catalog.ecosystems.len(), 1);
        let rust = &catalog.ecosystems[0];
        assert_eq!(rust.ecosystem, "rust");
        let names: Vec<&str> = rust.tasks.iter().map(|t| t.name.as_str()).collect();
        // Tasks now materialize from the config task table, so they surface in
        // the table's canonical (sorted) key order, not adapter insertion order.
        assert_eq!(names, ["bench", "format", "lint"]);
        // `bench` (a custom task) fans per module.
        assert_eq!(rust.tasks[0].fan_out, FanOut::PerModule);
        // Every task materialized from the config table is Project-origin.
        assert_eq!(rust.tasks[0].origin, TaskOrigin::Project);
        assert_eq!(rust.tasks[1].origin, TaskOrigin::Project);
        assert_eq!(catalog.names(), ["bench", "format", "lint"]);
    }

    #[test]
    fn groups_are_sorted_by_ecosystem() {
        let go = provider_with_tasks(
            "go",
            vec![Task::new(
                "test",
                vec!["go".into(), "test".into()],
                FanOut::PerModule,
            )],
        );
        let rust = provider_with_tasks(
            "rust",
            vec![Task::new(
                "build",
                vec!["cargo".into(), "build".into()],
                FanOut::Batchable,
            )],
        );
        let providers: Vec<&dyn Provider> = vec![&go, &rust];
        let catalog = task_catalog(&document_with(&["go", "rust"]), &providers).expect("catalog");
        let ecos: Vec<&str> = catalog
            .ecosystems
            .iter()
            .map(|e| e.ecosystem.as_str())
            .collect();
        assert_eq!(ecos, ["go", "rust"]);
    }
}
