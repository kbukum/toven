#![allow(missing_docs)]

//! The shared [`FakeCommandRunner`] records the [`LifecyclePolicy`] each
//! [`Invocation`] carried, proving the caller's lifecycle intent round-trips
//! intact across the [`CommandRunner`] port and that two distinct policies are
//! conveyed distinctly through the same seam.

use std::sync::Arc;
use std::time::Duration;

use rskit_process::LifecyclePolicy;
use tokio_util::sync::CancellationToken;
use toven_ports::{CommandRunner, Invocation};
use toven_testkit::FakeCommandRunner;

#[tokio::test]
async fn double_records_lifecycle_policy_round_tripped_through_the_port() {
    let runner = Arc::new(FakeCommandRunner::new());
    let policy = LifecyclePolicy::default()
        .with_grace_period(Duration::from_millis(250))
        .with_isolate_process_group(false);
    let invocation =
        Invocation::new("unit", vec!["true".to_string()]).with_lifecycle_policy(policy);

    CommandRunner::run(runner.as_ref(), &invocation, CancellationToken::new(), None)
        .await
        .expect("scripted run succeeds");

    assert_eq!(runner.lifecycles(), vec![("unit".to_string(), policy)]);
}

#[tokio::test]
async fn two_distinct_policies_are_conveyed_distinctly_through_the_port() {
    let runner = Arc::new(FakeCommandRunner::new());

    let interactive = LifecyclePolicy::default().with_grace_period(Duration::from_millis(100));
    let ci = LifecyclePolicy::default()
        .with_grace_period(Duration::from_secs(30))
        .with_isolate_process_group(false);
    assert_ne!(interactive, ci);

    CommandRunner::run(
        runner.as_ref(),
        &Invocation::new("interactive", vec!["true".to_string()])
            .with_lifecycle_policy(interactive),
        CancellationToken::new(),
        None,
    )
    .await
    .expect("interactive run succeeds");
    CommandRunner::run(
        runner.as_ref(),
        &Invocation::new("ci", vec!["true".to_string()]).with_lifecycle_policy(ci),
        CancellationToken::new(),
        None,
    )
    .await
    .expect("ci run succeeds");

    assert_eq!(
        runner.lifecycles(),
        vec![
            ("interactive".to_string(), interactive),
            ("ci".to_string(), ci),
        ]
    );
}
