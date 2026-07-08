//! Render one collapsed module group into a [`PlannedUnit`]: its argv, the facts
//! the Cache-decision phase folds into the content key, and the gating edges.

use std::collections::BTreeMap;
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use toven_model::{
    AbsPath, ExecutionReadiness, Module, ModuleKey, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{CommandTemplate, Readiness, TaskOrigin, TaskVar};

use super::grouping::group_dependencies;
use super::task::{EffectiveTask, effective_for};
use crate::plan::request::PlanRequest;

/// One scheduled, fully rendered unit awaiting its cache verdict.
///
/// Carries the execution facts (`argv`, `persistent`, `workspace`) plus the
/// keying facts (`base_argv`, `shared_inputs`, `cache_args`, `toolchain_identity`)
/// the Cache-decision phase folds into the content key.
#[derive(Debug, Clone)]
pub(in crate::plan) struct PlannedUnit {
    /// Stable unit id (`ecosystem:name#task`, member-prefixed under a federation;
    /// batched/whole-workspace units drop the module name and key by workspace:
    /// `ecosystem@workspace#task`, or `ecosystem#task` when workspace-less). A
    /// batch base in a cross-group cycle is split per layer, each tagged
    /// `~~L{layer}`.
    pub(in crate::plan) id: String,
    /// Representative module the unit operates on.
    pub(in crate::plan) module: ModuleKey,
    /// Every module collapsed into this unit (always non-empty, contains `module`).
    pub(in crate::plan) members: Vec<ModuleKey>,
    /// Name of the task this unit runs (its identity, the config table key).
    pub(in crate::plan) task: String,
    /// Provenance of the resolved task (which config layer won).
    pub(in crate::plan) origin: TaskOrigin,
    /// Owning workspace (keys the toolchain identity).
    pub(in crate::plan) workspace: Option<WorkspaceId>,
    /// Fully rendered argv (with passthrough spliced).
    pub(in crate::plan) argv: Vec<String>,
    /// Whether this unit starts a persistent process.
    pub(in crate::plan) persistent: bool,
    /// Persistent readiness signal.
    pub(in crate::plan) readiness: ExecutionReadiness,
    /// Persistent readiness timeout.
    pub(in crate::plan) readiness_timeout: Duration,
    /// Rendered base argv (without passthrough) — the `task_hash` source.
    pub(in crate::plan) base_argv: Vec<String>,
    /// Workspace-relative shared-input paths folded into the key.
    pub(in crate::plan) shared_inputs: Vec<String>,
    /// Whether passthrough args enter the key.
    pub(in crate::plan) cache_args: bool,
    /// Opaque `tool@version` identity for the owning workspace.
    pub(in crate::plan) toolchain_identity: String,
    /// Unit ids this unit depends on (scheduled dependency edges) for gating.
    pub(in crate::plan) depends_on: Vec<String>,
    /// Optional within-wave serialization key from the module metadata.
    pub(in crate::plan) resource_group: Option<String>,
}

/// Render a group of modules collapsed into one [`PlannedUnit`].
///
/// `members` is the set of modules sharing the group `id` in first-seen wave order
/// (a single module for `PerModule`, all same-ecosystem-and-workspace modules for
/// `Batchable`/`WholeWorkspace`, further split by layer only when the base is in a
/// cross-group cycle). Argv is rendered once from the representative member:
/// `Batchable` repeats each member's selector fragment, `WholeWorkspace` omits the
/// selector.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_unit(
    request: &PlanRequest,
    id: &str,
    members: &[ModuleKey],
    active_modules: &BTreeMap<ModuleKey, Module>,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
    workspaces: &BTreeMap<WorkspaceId, Workspace>,
    kept_deps: &BTreeMap<ModuleKey, Vec<ModuleKey>>,
    group_ids: &BTreeMap<ModuleKey, String>,
) -> AppResult<PlannedUnit> {
    let first = members.first().ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("scheduled group '{id}' has no members"),
        )
    })?;
    let representative = active_modules.get(first).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("scheduled unknown module '{first}'"),
        )
    })?;
    let task = &effective_for(first, effective)?.task;

    let template = CommandTemplate::parse(&task.argv, &task.selector)?;
    let modules = member_modules(members, active_modules)?;
    let workspaces_for = member_workspaces(&modules, workspaces)?;
    let argv = render_argv(
        &template,
        &request.passthrough,
        &modules,
        &workspaces_for,
        request,
    )?;
    let base_argv = render_argv(&template, &[], &modules, &workspaces_for, request)?;

    let toolchain_identity = resolve_toolchain_identity(representative, toolchain)?;
    let task_name = task.name.clone();
    crate::plan::shared_inputs::validate_shared_inputs(id, &task.shared_inputs)?;
    let depends_on = group_dependencies(id, members, kept_deps, group_ids);
    let resource_group = representative.resource_group.clone();

    Ok(PlannedUnit {
        id: id.to_string(),
        module: first.clone(),
        members: members.to_vec(),
        task: task_name,
        origin: task.origin,
        workspace: representative.workspace.clone(),
        argv,
        persistent: task.persistent,
        readiness: readiness(&task.readiness),
        readiness_timeout: task.readiness_timeout,
        base_argv,
        shared_inputs: task.shared_inputs.clone(),
        cache_args: task.cache_args,
        toolchain_identity,
        depends_on,
        resource_group,
    })
}

/// Resolve each member key to its module, failing closed on an unknown key.
fn member_modules<'a>(
    members: &[ModuleKey],
    active_modules: &'a BTreeMap<ModuleKey, Module>,
) -> AppResult<Vec<&'a Module>> {
    members
        .iter()
        .map(|key| {
            active_modules.get(key).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("scheduled unknown module '{key}'"),
                )
            })
        })
        .collect()
}

/// Resolve each module's owning workspace (none when the module is workspace-less).
fn member_workspaces<'a>(
    modules: &[&Module],
    workspaces: &'a BTreeMap<WorkspaceId, Workspace>,
) -> AppResult<Vec<Option<&'a Workspace>>> {
    modules
        .iter()
        .map(|module| {
            module.workspace.as_ref().map_or_else(
                || Ok(None),
                |id| {
                    workspaces.get(id).map(Some).ok_or_else(|| {
                        AppError::new(
                            rskit_errors::ErrorCode::Internal,
                            format!("module '{}' references unknown workspace '{id}'", module.id),
                        )
                    })
                },
            )
        })
        .collect()
}

/// The `tool@version` cache identity for the representative module's workspace.
fn resolve_toolchain_identity(
    representative: &Module,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
) -> AppResult<String> {
    let Some(id) = &representative.workspace else {
        return Ok(String::new());
    };
    let tag = toolchain.get(id).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "module '{}' workspace '{id}' has no resolved toolchain identity",
                representative.id
            ),
        )
    })?;
    Ok(toolchain_identity(tag))
}

/// The `tool@version` cache identity for a resolved toolchain tag.
fn toolchain_identity(tag: &ToolchainTag) -> String {
    format!("{}@{}", tag.tool, tag.version.as_deref().unwrap_or(""))
}

/// Render a group's argv, batching every member's selector fragment.
fn render_argv(
    template: &CommandTemplate,
    passthrough: &[String],
    modules: &[&Module],
    workspaces: &[Option<&Workspace>],
    request: &PlanRequest,
) -> AppResult<Vec<String>> {
    let mut resolvers: Vec<_> = modules
        .iter()
        .zip(workspaces)
        .map(|(module, workspace)| {
            move |var: TaskVar| resolve(var, module, *workspace, &request.project_root)
        })
        .collect();
    template.render_batch(passthrough, &mut resolvers)
}

/// Convert the adapter task readiness vocabulary into immutable plan vocabulary.
fn readiness(readiness: &Readiness) -> ExecutionReadiness {
    match readiness {
        Readiness::Started => ExecutionReadiness::Started,
        Readiness::Command(argv) => ExecutionReadiness::Command(argv.clone()),
        Readiness::OutputContains(value) => ExecutionReadiness::OutputContains(value.clone()),
    }
}

/// Resolve one non-splice [`TaskVar`] for a module's argv render.
fn resolve(
    var: TaskVar,
    module: &Module,
    workspace: Option<&Workspace>,
    project_root: &AbsPath,
) -> AppResult<String> {
    let value = match var {
        TaskVar::ProjectRoot => project_root.as_path().display().to_string(),
        TaskVar::WorkspaceRoot => workspace
            .ok_or_else(|| {
                AppError::invalid_input(
                    "task.argv",
                    format!(
                        "module '{}' has no workspace for '{{workspace.root}}'",
                        module.id
                    ),
                )
            })?
            .root
            .as_path()
            .display()
            .to_string(),
        TaskVar::ModuleName => module.id.name.clone(),
        TaskVar::ModulePackage => module
            .package
            .clone()
            .unwrap_or_else(|| module.id.name.clone()),
        TaskVar::ModuleRoot => module.root.as_path().display().to_string(),
        TaskVar::ModuleManifest => module
            .manifest
            .as_ref()
            .ok_or_else(|| {
                AppError::invalid_input(
                    "task.argv",
                    format!(
                        "module '{}' has no manifest for '{{module.manifest}}'",
                        module.id
                    ),
                )
            })?
            .as_path()
            .display()
            .to_string(),
        TaskVar::ModuleSelector | TaskVar::Args => {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!("splice variable '{var}' must be handled by the template, not resolved"),
            ));
        }
    };
    Ok(value)
}
