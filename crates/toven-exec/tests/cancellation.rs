#![allow(missing_docs)]
#![cfg(unix)]

//! A normal run stopped through its cancellation token reports a cancelled
//! outcome, not a failed exit, so `--fail-fast` and Ctrl+C never count the
//! units they stop as failures.

use std::time::Duration;

use rskit_process::LifecyclePolicy;
use rskit_testutil::TestWorkspace;
use tokio_util::sync::CancellationToken;
use toven_exec::ProcessCommandRunner;
use toven_ports::{CommandRunner, Invocation, OutputObserver};

fn sleeper() -> Invocation {
    Invocation::new(
        "sleeper",
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo started; exec sleep 30".to_string(),
        ],
    )
    .with_lifecycle_policy(LifecyclePolicy::default().with_grace_period(Duration::from_millis(100)))
}

async fn cancel_after_start(live: Option<OutputObserver>) -> toven_ports::RunOutcome {
    let workspace = TestWorkspace::new("exec-cancel");
    let runner = ProcessCommandRunner::new(workspace.path());
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });
    runner
        .run(&sleeper(), cancel, live)
        .await
        .expect("a cancelled run is an outcome, not an error")
}

#[tokio::test]
async fn a_cancelled_captured_run_is_reported_cancelled() {
    let outcome = cancel_after_start(None).await;
    assert!(outcome.cancelled);
    assert!(!outcome.success);
    assert!(
        !outcome.output.is_empty(),
        "output captured before the cancel is kept"
    );
}

#[tokio::test]
async fn a_cancelled_streamed_run_is_reported_cancelled() {
    let (sender, _receiver) = tokio::sync::mpsc::channel(16);
    let outcome = cancel_after_start(Some(OutputObserver::new(sender))).await;
    assert!(outcome.cancelled);
    assert!(!outcome.success);
}

#[tokio::test]
async fn a_run_that_finishes_is_not_cancelled() {
    let workspace = TestWorkspace::new("exec-not-cancelled");
    let runner = ProcessCommandRunner::new(workspace.path());
    let invocation = Invocation::new(
        "fails",
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "exit 3".to_string(),
        ],
    );
    let outcome = runner
        .run(&invocation, CancellationToken::new(), None)
        .await
        .expect("run");
    assert!(!outcome.cancelled);
    assert_eq!(outcome.exit_code, Some(3));
}
