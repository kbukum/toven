#![cfg(test)]

use std::time::Duration;

use crate::{
    core::{CommandOrigin, ExecutionMode, ExecutionUnit, PersistentReadiness},
    exec::RunOptions,
};

use super::lifecycle::start_persistent_execution_unit;

#[test]
fn output_matcher_marks_persistent_unit_ready() {
    let root = rskit_testutil::test_workspace!("persistent-ready-output");
    let mut unit = unit();
    unit.argv_template = vec![
        "sh".to_string(),
        "-c".to_string(),
        "printf listening; sleep 2".to_string(),
    ];
    unit.readiness = PersistentReadiness::OutputContains("listening".to_string());

    let persistent = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    )
    .expect("persistent unit becomes ready");

    assert!(persistent.output.result.stdout.contains("listening"));
}

#[test]
fn shutdown_accepts_successful_process_that_already_exited() {
    let root = rskit_testutil::test_workspace!("persistent-shutdown-already-exited");
    let mut unit = unit();
    unit.argv_template = vec![
        "sh".to_string(),
        "-c".to_string(),
        "printf ready".to_string(),
    ];
    unit.readiness = PersistentReadiness::OutputContains("ready".to_string());

    let persistent = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    )
    .expect("persistent unit becomes ready");

    persistent.process.shutdown().expect("shutdown succeeds");
}

#[test]
fn readiness_timeout_fails_persistent_unit() {
    let root = rskit_testutil::test_workspace!("persistent-ready-timeout");
    let mut unit = unit();
    unit.readiness = PersistentReadiness::OutputContains("never".to_string());
    unit.readiness_timeout = Duration::from_millis(20);

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("readiness should time out");
    };

    assert_eq!(error.code, crate::core::ErrorCode::Timeout);
}

#[test]
fn early_exit_before_readiness_reports_process_status() {
    let root = rskit_testutil::test_workspace!("persistent-early-exit-before-ready");
    let mut unit = unit();
    unit.argv_template = vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()];
    unit.readiness = PersistentReadiness::OutputContains("never".to_string());

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("early exit should fail readiness");
    };

    assert_eq!(error.code, crate::core::ErrorCode::Internal);
    assert!(
        error.message.contains("exited unexpectedly")
            || error
                .message
                .contains("output ended before readiness was observed"),
        "unexpected error message: {}",
        error.message
    );
}

#[test]
fn empty_persistent_argv_reports_invalid_input() {
    let root = rskit_testutil::test_workspace!("persistent-empty-argv");
    let mut unit = unit();
    unit.argv_template = Vec::new();

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("empty argv should fail");
    };

    assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
    assert!(error.message.contains("scopes.dev.tasks.server.argv"));
    assert!(error.message.contains("rendered an empty argv"));
}

#[test]
fn spawn_failure_reports_persistent_unit() {
    let root = rskit_testutil::test_workspace!("persistent-spawn-failure");
    let mut unit = unit();
    unit.argv_template = vec!["/definitely/not/a/toven-command".to_string()];

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("spawn failure should fail");
    };

    assert_eq!(error.code, crate::core::ErrorCode::Internal);
    assert!(error.message.contains("failed to spawn persistent unit"));
}

#[test]
fn readiness_command_success_marks_unit_ready() {
    let root = rskit_testutil::test_workspace!("persistent-ready-command-success");
    let mut unit = unit();
    unit.argv_template = vec!["sleep".to_string(), "2".to_string()];
    unit.readiness = PersistentReadiness::Command(vec!["true".to_string()]);

    let persistent = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    )
    .expect("readiness command succeeds");

    persistent.process.shutdown().expect("process shuts down");
}

#[test]
fn readiness_command_failure_reports_failure() {
    let root = rskit_testutil::test_workspace!("persistent-ready-command-failure");
    let mut unit = unit();
    unit.readiness = PersistentReadiness::Command(vec!["false".to_string()]);

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("readiness command should fail");
    };

    assert_eq!(error.code, crate::core::ErrorCode::Internal);
    assert!(error.message.contains("readiness command failed"));
}

#[test]
fn readiness_command_render_errors_reference_ready_command() {
    let root = rskit_testutil::test_workspace!("persistent-ready-command-render-error");
    let mut unit = unit();
    unit.readiness = PersistentReadiness::Command(vec!["{args}-bad".to_string()]);

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("readiness command render should fail");
    };

    assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
    assert!(error.message.contains("ready_command"));
    assert!(!error.message.contains(".argv"));
}

#[test]
fn empty_readiness_command_reports_ready_command_field() {
    let root = rskit_testutil::test_workspace!("persistent-empty-ready-command");
    let mut unit = unit();
    unit.readiness = PersistentReadiness::Command(Vec::new());

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );
    let Err(error) = result else {
        panic!("empty readiness command should fail");
    };

    assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
    assert!(
        error
            .message
            .contains("scopes.dev.tasks.server.ready_command")
    );
    assert!(!error.message.contains(".argv"));
}

#[test]
fn readiness_command_uses_readiness_timeout() {
    let root = rskit_testutil::test_workspace!("persistent-ready-command-timeout");
    let mut unit = unit();
    unit.readiness = PersistentReadiness::Command(vec!["sleep".to_string(), "2".to_string()]);
    unit.readiness_timeout = Duration::from_millis(20);

    let result = start_persistent_execution_unit(
        &unit,
        root.path(),
        &RunOptions {
            timeout: None,
            cancel_on_ctrl_c: false,
            cancellation: None,
        },
    );

    let Err(error) = result else {
        panic!("readiness command should time out");
    };
    assert_eq!(error.code, crate::core::ErrorCode::Timeout);
    assert!(error.message.contains("readiness command timed out"));
}

fn unit() -> ExecutionUnit {
    ExecutionUnit {
        id: "dev/server/workspace".to_string(),
        scope_id: crate::core::ScopeId::new("dev").expect("scope id"),
        adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
        task: "server".to_string(),
        command_origin: CommandOrigin::DirectArgv,
        task_origin: crate::core::TaskOrigin::ProjectDefault,
        mode: ExecutionMode::WorkspaceOnce,
        resource_group: String::new(),
        modules: Vec::new(),
        argv_template: vec!["sh".to_string(), "-c".to_string(), "sleep 2".to_string()],
        module_arg_template: Vec::new(),
        passthrough_args: Vec::new(),
        cache_args: false,
        persistent: true,
        readiness: PersistentReadiness::Started,
        readiness_timeout: Duration::from_secs(2),
        shared_inputs: Vec::new(),
    }
}
