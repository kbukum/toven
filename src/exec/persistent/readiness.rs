use std::path::Path;

use crate::{
    core::{AppError, AppResult, ExecutionUnit, PersistentReadiness},
    exec::render_execution_unit,
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
            let argv = render_execution_unit(&ready_unit, workspace_root)?;
            let command = command_from_argv(&argv, workspace_root).map_err(|()| {
                AppError::invalid_input(
                    "readiness.argv",
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
