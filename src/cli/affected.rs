//! `toven affected` command.

use std::{collections::BTreeMap, io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    config::load_workspace,
    core::{AppError, AppResult, Module, ModuleId, Workspace},
    engine::{
        DiscoveredTaskProfile,
        affected::{ChangedPath, affected_modules},
        discover_workspace_task_profiles,
    },
    git::{
        affected::changed_paths,
        baseline::{
            BaselineContext, BaselineProvider, ExplicitBaselineProvider, GitRefBaselineProvider,
            MergeBaseBaselineProvider,
        },
    },
};

pub(super) fn run_affected(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the affected config default"),
    );
    let task = matches
        .get_one::<String>("task")
        .expect("clap supplies the affected task default")
        .as_str();
    let workspace = load_workspace(config)?;
    let changes = resolve_affected_changes(&workspace, matches)?;
    let discovered =
        discover_workspace_task_profiles(&workspace, task, &AdapterRegistry::default())?;
    let modules = modules_from_discovered(&discovered)?;
    let affected = resolve_affected_modules(changes, &modules)?;

    writeln!(
        stdout,
        "baseline: {} {}",
        affected.provider, affected.baseline_oid
    )
    .map_err(AppError::internal)?;
    if affected.changed_paths.is_empty() {
        writeln!(stdout, "changed_paths: none").map_err(AppError::internal)?;
    } else {
        writeln!(stdout, "changed_paths:").map_err(AppError::internal)?;
        for path in &affected.changed_paths {
            writeln!(stdout, "- {}", path.display()).map_err(AppError::internal)?;
        }
    }
    if affected.closure.is_empty() {
        writeln!(stdout, "modules: none").map_err(AppError::internal)?;
    } else {
        writeln!(stdout, "modules:").map_err(AppError::internal)?;
        for module in &affected.closure {
            let reason = if !affected.global_paths.is_empty() {
                "global"
            } else if affected.direct.contains(module) {
                "direct"
            } else {
                "dependent"
            };
            writeln!(stdout, "- {module} ({reason})").map_err(AppError::internal)?;
        }
    }
    Ok(())
}

pub(super) struct CliAffectedModules {
    pub(super) provider: String,
    pub(super) baseline_oid: String,
    pub(super) changed_paths: Vec<PathBuf>,
    pub(super) global_paths: Vec<PathBuf>,
    pub(super) direct: std::collections::BTreeSet<ModuleId>,
    pub(super) closure: std::collections::BTreeSet<ModuleId>,
}

pub(super) struct CliAffectedChanges {
    provider: String,
    baseline_oid: String,
    changed: Vec<ChangedPath>,
}

pub(super) fn resolve_affected_changes(
    workspace: &Workspace,
    matches: &ArgMatches,
) -> AppResult<CliAffectedChanges> {
    let provider = baseline_provider(workspace, matches)?;
    let baseline = provider.resolve(&BaselineContext {
        workspace_root: workspace.root.clone(),
    })?;
    let changed = changed_paths(workspace, &baseline)?;

    Ok(CliAffectedChanges {
        provider: baseline.provider,
        baseline_oid: baseline.oid,
        changed,
    })
}

pub(super) fn resolve_affected_modules(
    changes: CliAffectedChanges,
    modules: &[Module],
) -> AppResult<CliAffectedModules> {
    let affected = affected_modules(modules, &changes.changed)?;

    Ok(CliAffectedModules {
        provider: changes.provider,
        baseline_oid: changes.baseline_oid,
        changed_paths: changes.changed.into_iter().map(|path| path.path).collect(),
        global_paths: affected.global_paths,
        direct: affected.direct,
        closure: affected.closure,
    })
}

fn baseline_provider(
    workspace: &Workspace,
    matches: &ArgMatches,
) -> AppResult<Box<dyn BaselineProvider>> {
    let explicit_base = matches.get_one::<String>("base").cloned();
    let base = explicit_base.clone().or_else(|| workspace.base_ref.clone());
    let merge_base = matches.get_flag("merge-base");

    match (base, merge_base, explicit_base.is_some()) {
        (Some(base), true, _) => Ok(Box::new(MergeBaseBaselineProvider::new(base))),
        (Some(base), false, true) => Ok(Box::new(ExplicitBaselineProvider::new(base))),
        (Some(base), false, false) => Ok(Box::new(GitRefBaselineProvider::new(base))),
        (None, true, _) => Err(AppError::invalid_input(
            "base",
            "--merge-base requires --base or workspace.base_ref",
        )),
        (None, false, _) => Ok(Box::new(ExplicitBaselineProvider::new("HEAD"))),
    }
}

pub(super) fn modules_from_discovered(
    discovered: &[DiscoveredTaskProfile],
) -> AppResult<Vec<Module>> {
    let mut modules = BTreeMap::new();
    for profile in discovered {
        for module in &profile.modules {
            if let Some(existing) = modules.get(&module.name) {
                if existing != module {
                    return Err(AppError::invalid_input(
                        "modules",
                        format!(
                            "profile '{}' discovered conflicting definition for module '{}'",
                            profile.profile.name, module.name
                        ),
                    ));
                }
                continue;
            }
            modules.insert(module.name.clone(), module.clone());
        }
    }
    Ok(modules.into_values().collect())
}
