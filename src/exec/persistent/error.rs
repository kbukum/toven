use crate::core::{AppError, ErrorCode, ExecutionUnit};

pub(super) fn remap_start_error(unit: &ExecutionUnit, error: AppError) -> AppError {
    let Some(kind) = rskit_process::persistent_start_error_kind(&error) else {
        return error;
    };
    match kind {
        rskit_process::PersistentStartErrorKind::SpawnFailed => AppError::new(
            error.code,
            format!("failed to spawn persistent unit '{}'", unit.id),
        )
        .with_cause(error),
        rskit_process::PersistentStartErrorKind::ReadinessCommandTimedOut => AppError::new(
            ErrorCode::Timeout,
            format!("persistent unit '{}' readiness command timed out", unit.id),
        )
        .with_cause(error),
        rskit_process::PersistentStartErrorKind::ReadinessCommandFailed => AppError::new(
            ErrorCode::Internal,
            format!("persistent unit '{}' readiness command failed", unit.id),
        )
        .with_cause(error),
        rskit_process::PersistentStartErrorKind::ReadinessTimedOut => AppError::new(
            ErrorCode::Timeout,
            format!("persistent unit '{}' did not become ready", unit.id),
        )
        .with_cause(error),
        rskit_process::PersistentStartErrorKind::OutputEndedBeforeReadiness => AppError::new(
            ErrorCode::Internal,
            format!(
                "persistent unit '{}' output ended before readiness was observed",
                unit.id
            ),
        )
        .with_cause(error),
        rskit_process::PersistentStartErrorKind::ExitedBeforeReadiness => AppError::new(
            ErrorCode::Internal,
            format!("persistent unit '{}' exited unexpectedly", unit.id),
        )
        .with_cause(error),
        _ => error,
    }
}

pub(super) fn persistent_exit_result_error(
    unit_id: &str,
    result: &rskit_process::ProcessResult,
) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!(
            "persistent unit '{unit_id}' exited unexpectedly with status {:?}",
            result.exit_code
        ),
    )
}
