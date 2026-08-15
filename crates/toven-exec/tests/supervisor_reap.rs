#![allow(missing_docs)]
#![cfg(unix)]

//! The concrete runners register every spawned child with an injected
//! [`ProcessSupervisor`], so a process-level shutdown reaps the whole group as
//! the backstop — even when nothing observes cooperative cancellation.
//!
//! Mirrors the rskit `supervisor_supervised_run` shape one layer up: the
//! runner spawns a shell that backgrounds a long `sleep` in its own process
//! group and records the grandchild pid, then a scripted `shutdown()` must
//! reap it structurally.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rskit_process::{LifecyclePolicy, ProcessSupervisor};
use rskit_testutil::TestWorkspace;
use tokio_util::sync::CancellationToken;
use toven_exec::{ProcessCommandRunner, ProcessToolRunner};
use toven_model::ExecutionReadiness;
use toven_ports::{
    CommandRunner, Invocation, OutputObserver, StartOutcome, ToolInvocation, ToolRunner,
};

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether `pid` names a process that is still *running* — present in the
/// process table and not a zombie. A `SIGKILL`ed child whose owner has not yet
/// reaped it lingers as a zombie that still answers `kill -0`, so a liveness
/// assertion must exclude the `Z` state to tell "still running" from "killed but
/// not yet reaped".
fn pid_running(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .is_some_and(|stat| !stat.is_empty() && !stat.starts_with('Z'))
}

fn read_pid_blocking(path: &Path) -> u32 {
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pid file never became a valid pid: {}", path.display());
}

async fn read_pid_async(path: &Path) -> u32 {
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("pid file never became a valid pid: {}", path.display());
}

async fn wait_until_gone(pid: u32) -> bool {
    for _ in 0..250 {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !pid_alive(pid)
}

async fn wait_for_registration(supervisor: &ProcessSupervisor) {
    for _ in 0..500 {
        if supervisor.registry_len() >= 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("child never registered with the injected supervisor");
}

fn group_sleeper_argv(pid_file: &Path) -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "sleep 30 >/dev/null 2>&1 & printf %s \"$!\" > '{}'; wait",
            pid_file.display()
        ),
    ]
}

fn short_grace() -> LifecyclePolicy {
    LifecyclePolicy::default().with_grace_period(Duration::from_millis(100))
}

/// A persistent process that records its own pid and then blocks, so a test can
/// prove the child outlives the runner while a held handle still owns it.
fn persistent_pid_argv(pid_file: &Path) -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("printf %s \"$$\" > '{}'; exec sleep 30", pid_file.display()),
    ]
}

#[tokio::test]
async fn command_runner_registers_children_and_shutdown_reaps_the_group() {
    let workspace = TestWorkspace::new("exec-command-reap");
    let pid_file = workspace.child("gc.pid").expect("pid path");
    let supervisor = Arc::new(ProcessSupervisor::new(short_grace()));

    let runner =
        ProcessCommandRunner::new(workspace.path()).with_supervisor(Arc::clone(&supervisor));
    let invocation = Invocation::new("sleeper", group_sleeper_argv(&pid_file))
        .with_lifecycle_policy(short_grace());

    let run = tokio::spawn(async move {
        let _ = runner
            .run(&invocation, CancellationToken::new(), None)
            .await;
    });

    wait_for_registration(&supervisor).await;
    let grandchild = read_pid_async(&pid_file).await;

    supervisor
        .shutdown("test backstop")
        .await
        .expect("shutdown");

    assert!(
        wait_until_gone(grandchild).await,
        "the injected supervisor must reap the command runner's process group"
    );
    let _ = run.await;
    assert_eq!(supervisor.registry_len(), 0);
}

#[tokio::test]
async fn tool_runner_registers_children_and_shutdown_reaps_the_group() {
    let workspace = TestWorkspace::new("exec-tool-reap");
    let pid_file = workspace.child("gc.pid").expect("pid path");
    let supervisor = Arc::new(ProcessSupervisor::new(short_grace()));

    let runner = ProcessToolRunner::new().with_supervisor(Arc::clone(&supervisor));
    let invocation = ToolInvocation::new(group_sleeper_argv(&pid_file));

    let run = tokio::task::spawn_blocking(move || {
        let _ = runner.run(&invocation);
    });

    wait_for_registration(&supervisor).await;
    let pid_path = pid_file.clone();
    let grandchild = tokio::task::spawn_blocking(move || read_pid_blocking(&pid_path))
        .await
        .expect("read pid");

    supervisor
        .shutdown("test backstop")
        .await
        .expect("shutdown");

    assert!(
        wait_until_gone(grandchild).await,
        "the injected supervisor must reap the tool runner's process group"
    );
    let _ = run.await;
    assert_eq!(supervisor.registry_len(), 0);
}

#[tokio::test]
async fn a_held_persistent_process_survives_dropping_the_runner_that_started_it() {
    // Regression: a supervised persistent process owns no supervisor of its own —
    // its registration lives in the runner's supervisor registry. The returned
    // held handle must keep that supervisor alive, so dropping the runner before
    // the caller shuts the handle down must NOT drain the registration and reap
    // the still-owned child. Use the runner's own private supervisor (no external
    // Arc) so the runner would otherwise hold the last reference.
    let workspace = TestWorkspace::new("exec-persistent-runner-drop");
    let pid_file = workspace.child("held.pid").expect("pid path");
    let runner = ProcessCommandRunner::new(workspace.path());
    let invocation = Invocation::new("held", persistent_pid_argv(&pid_file))
        .with_persistent(true)
        .with_readiness(ExecutionReadiness::Started);

    let outcome = runner
        .start_persistent(
            &invocation,
            CancellationToken::new(),
            OutputObserver::none(),
        )
        .await
        .expect("persistent start");
    let StartOutcome::Ready { process, .. } = outcome else {
        panic!("persistent process should reach readiness");
    };

    let child = read_pid_async(&pid_file).await;
    assert!(pid_running(child), "the persistent child is running");

    // Drop the runner (and its supervisor) while the handle is still held.
    drop(runner);

    // The child must still be running: the held handle kept the supervisor's
    // registry from draining it. (A dropped supervisor would `SIGKILL` the
    // group, leaving a zombie that still answers `kill -0`, so assert on the
    // running state rather than mere presence.)
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        pid_running(child),
        "dropping the runner must not reap a persistent child the caller still holds"
    );

    // Explicit teardown through the held handle reaps it as usual.
    tokio::task::spawn_blocking(move || process.shutdown())
        .await
        .expect("join shutdown")
        .expect("held process shuts down");
    assert!(
        wait_until_gone(child).await,
        "shutting the held handle down reaps the persistent child"
    );
}
