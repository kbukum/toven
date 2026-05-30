use std::path::Path;

use crate::{
    core::{AppError, AppResult, ExecutionUnit, PersistentReadiness},
    exec::{render::argv_field, render_execution_unit},
};

use super::command::command_from_argv;

pub(super) fn readiness(
    unit: &ExecutionUnit,
    workspace_root: &Path,
) -> AppResult<rskit_process::PersistentReadiness> {
    match &unit.readiness {
        PersistentReadiness::Started => Ok(rskit_process::PersistentReadiness::Started),
        PersistentReadiness::OutputContains(value) => Ok(
            rskit_process::PersistentReadiness::OutputContains(value.clone()),
        ),
        PersistentReadiness::Command(argv_template) => {
            let mut ready_unit = unit.clone();
            ready_unit.argv_template.clone_from(argv_template);
            let argv = render_execution_unit(&ready_unit, workspace_root)
                .map_err(|error| remap_ready_command_error(unit, error))?;
            let command = command_from_argv(&argv, workspace_root).map_err(|()| {
                AppError::invalid_input(
                    ready_command_field(unit),
                    format!(
                        "persistent unit '{}' rendered an empty readiness argv",
                        unit.id
                    ),
                )
            })?;
            Ok(rskit_process::PersistentReadiness::Command(command))
        }
    }
}

fn remap_ready_command_error(unit: &ExecutionUnit, error: AppError) -> AppError {
    let argv_prefix = format!("invalid {}: ", argv_field(unit));
    match error.message.strip_prefix(&argv_prefix) {
        Some(reason) => {
            AppError::invalid_input(ready_command_field(unit), reason.to_string()).with_cause(error)
        }
        None => error,
    }
}

fn ready_command_field(unit: &ExecutionUnit) -> String {
    format!(
        "profiles.{}.tasks.{}.ready_command",
        unit.profile, unit.task
    )
}
