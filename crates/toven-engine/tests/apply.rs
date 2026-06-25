//! APPLY pipeline tests over scriptable command/cache/output doubles.

use std::sync::Arc;
use std::time::Duration;

use toven_engine::apply::{ApplyOptions, apply};
use toven_engine::output::UnitOutputChannel;
use toven_model::{
    CacheVerdict, EcosystemId, Event, ExecutionReadiness, ExecutionUnit, ModuleRef, OutputStream,
    Plan, UnitOutput, UnitStatus,
};
use toven_ports::CommandRunner;
use toven_testkit::{
    FakeCommandRunner, RecordingCacheWriter, RecordingRawOutputSink, RecordingReporter,
    TestWorkspace,
};

fn mref(name: &str) -> ModuleRef {
    ModuleRef::new(EcosystemId::new("rust").expect("ecosystem"), name).expect("module ref")
}

fn unit(id: &str) -> ExecutionUnit {
    ExecutionUnit {
        id: id.to_string(),
        module: mref(id),
        kind: "test".to_string(),
        workspace: None,
        argv: vec!["fake".to_string(), id.to_string()],
        persistent: false,
        readiness: ExecutionReadiness::Started,
        readiness_timeout: Duration::from_secs(30),
        cache: CacheVerdict::Miss,
        cache_key: Some(format!("key-{id}")),
        depends_on: Vec::new(),
        resource_group: None,
    }
}

fn run(
    plan: &Plan,
    runner: &Arc<FakeCommandRunner>,
    cache: &RecordingCacheWriter,
    reporter: &mut RecordingReporter,
) -> toven_model::RunStats {
    let runner_port: Arc<dyn CommandRunner> = runner.clone();
    let sink = RecordingRawOutputSink::new();
    let mut output = UnitOutputChannel::new(sink);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    runtime
        .block_on(apply(
            plan,
            runner_port,
            cache,
            reporter,
            &mut output,
            ApplyOptions {
                max_parallel: 2,
                fail_fast: false,
                ..ApplyOptions::default()
            },
            tokio_util::sync::CancellationToken::new(),
        ))
        .expect("apply succeeds")
}

fn run_with_sink(
    plan: &Plan,
    runner: Arc<FakeCommandRunner>,
    cache: &RecordingCacheWriter,
    reporter: &mut RecordingReporter,
    sink: RecordingRawOutputSink,
) -> toven_model::RunStats {
    let runner_port: Arc<dyn CommandRunner> = runner;
    let mut output = UnitOutputChannel::new(sink);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    runtime
        .block_on(apply(
            plan,
            runner_port,
            cache,
            reporter,
            &mut output,
            ApplyOptions {
                max_parallel: 2,
                fail_fast: false,
                ..ApplyOptions::default()
            },
            tokio_util::sync::CancellationToken::new(),
        ))
        .expect("apply succeeds")
}

fn chunk(unit_id: &str, stream: OutputStream, bytes: &[u8]) -> UnitOutput {
    UnitOutput {
        unit_id: unit_id.to_string(),
        stream,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn default_apply_environment_is_explicit_path_allowlist_only() {
    let options = ApplyOptions::default();

    assert_eq!(
        options.environment.policy,
        toven_ports::InvocationEnvPolicy::ExplicitOnly
    );
    assert!(
        options
            .environment
            .vars
            .keys()
            .all(|key| key.as_str() == "PATH")
    );
}

#[test]
fn cache_hits_skip_execution_and_successes_record_cache() {
    let mut hit = unit("hit");
    hit.cache = CacheVerdict::Hit;
    hit.cache_key = None;
    let miss = unit("miss");
    let plan = Plan::new(vec![hit, miss], vec![vec!["hit".into(), "miss".into()]]);
    let runner = Arc::new(FakeCommandRunner::new());
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    let stats = run(&plan, &runner, &cache, &mut reporter);

    assert_eq!(runner.started(), vec!["miss".to_string()]);
    assert_eq!(cache.recorded(), vec!["key-miss".to_string()]);
    assert_eq!(stats.cached_units, 1);
    assert_eq!(stats.ran_units, 1);
}

#[test]
fn resource_groups_serialize_within_group_but_run_across_groups() {
    let mut a = unit("a");
    a.resource_group = Some("shared".to_string());
    let mut b = unit("b");
    b.resource_group = Some("shared".to_string());
    let c = unit("c");
    let plan = Plan::new(
        vec![a, b, c],
        vec![vec!["a".into(), "b".into(), "c".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new());
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run(&plan, &runner, &cache, &mut reporter);

    assert!(
        runner.peak_concurrency() >= 2,
        "different groups should overlap"
    );
    for (unit, coactive) in runner.coactive() {
        if unit == "a" {
            assert!(!coactive.contains(&"b".to_string()));
        }
        if unit == "b" {
            assert!(!coactive.contains(&"a".to_string()));
        }
    }
}

#[test]
fn keep_going_blocks_reverse_dependents_but_runs_independents() {
    let a = unit("a");
    let x = unit("x");
    let mut b = unit("b");
    b.depends_on = vec!["a".to_string()];
    let plan = Plan::new(
        vec![a, x, b],
        vec![vec!["a".into(), "x".into()], vec!["b".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_failure("a"));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    let stats = run(&plan, &runner, &cache, &mut reporter);

    assert_eq!(runner.started(), vec!["a".to_string(), "x".to_string()]);
    assert_eq!(stats.failed_units, 1);
    assert_eq!(stats.blocked_units, 1);
    assert!(reporter.events().iter().any(|event| {
        matches!(
            event,
            Event::UnitFinished {
                unit_id,
                status: UnitStatus::Blocked
            } if unit_id == "b"
        )
    }));
}

#[test]
fn fail_fast_cancels_in_flight_and_stops_later_waves() {
    let a = unit("a");
    let b = unit("b");
    let c = unit("c");
    let plan = Plan::new(
        vec![a, b, c],
        vec![vec!["a".into(), "b".into()], vec!["c".into()]],
    );
    let runner = Arc::new(
        FakeCommandRunner::new()
            .with_failure("a")
            .with_blocking("b"),
    );
    let runner_port: Arc<dyn CommandRunner> = runner.clone();
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();
    let sink = RecordingRawOutputSink::new();
    let mut output = UnitOutputChannel::new(sink);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");

    let stats = runtime
        .block_on(apply(
            &plan,
            runner_port,
            &cache,
            &mut reporter,
            &mut output,
            ApplyOptions {
                max_parallel: 2,
                fail_fast: true,
                ..ApplyOptions::default()
            },
            tokio_util::sync::CancellationToken::new(),
        ))
        .expect("apply succeeds");

    assert!(stats.has_failures());
    assert_eq!(runner.cancelled(), vec!["b".to_string()]);
    assert!(!runner.started().contains(&"c".to_string()));

    // Every planned unit reaches a terminal event: `a` failed, while the
    // in-flight-cancelled `b` and the never-scheduled later-wave `c` both get a
    // terminal `Cancelled` event so the stream accounts for all planned units.
    assert_eq!(stats.cancelled_units, 2);
    let cancelled: Vec<&String> = reporter
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::UnitFinished {
                unit_id,
                status: UnitStatus::Cancelled,
            } => Some(unit_id),
            _ => None,
        })
        .collect();
    assert_eq!(cancelled, vec!["b", "c"]);
}

#[test]
fn external_cancel_tears_down_in_flight_and_stops_later_waves() {
    // `a` blocks in its first wave; `b` sits in a later wave. Once `a` is
    // in-flight an external cancel (the Ctrl+C wiring) fires, which must run the
    // same cooperative teardown a `--fail-fast` failure does even though
    // `fail_fast` is false here: SIGTERM the in-flight worker, stop scheduling
    // later waves, drain held processes / shut the pool down (no orphan), and
    // still return aggregated `RunStats` with terminal `Cancelled` events.
    let a = unit("a");
    let b = unit("b");
    let plan = Plan::new(vec![a, b], vec![vec!["a".into()], vec!["b".into()]]);
    let runner = Arc::new(FakeCommandRunner::new().with_blocking("a"));
    let runner_port: Arc<dyn CommandRunner> = runner.clone();
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();
    let sink = RecordingRawOutputSink::new();
    let mut output = UnitOutputChannel::new(sink);
    let cancel = tokio_util::sync::CancellationToken::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");

    let stats = runtime.block_on(async {
        // Fire the external cancel only once `a` is actually in-flight, so the
        // run exercises the live-worker teardown branch rather than the
        // pre-scheduling short-circuit.
        let watcher_token = cancel.clone();
        let watcher_runner = runner.clone();
        let watcher = tokio::spawn(async move {
            loop {
                if watcher_runner.started().contains(&"a".to_string()) {
                    watcher_token.cancel();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let stats = apply(
            &plan,
            runner_port,
            &cache,
            &mut reporter,
            &mut output,
            ApplyOptions {
                max_parallel: 2,
                fail_fast: false,
                ..ApplyOptions::default()
            },
            cancel,
        )
        .await
        .expect("apply succeeds");
        watcher.await.expect("watcher joins");
        stats
    });

    // `a` observed cancellation while in-flight; the later-wave `b` never started.
    assert_eq!(runner.cancelled(), vec!["a".to_string()]);
    assert!(!runner.started().contains(&"b".to_string()));

    // Both the in-flight-cancelled `a` and the never-scheduled `b` reach a
    // terminal `Cancelled` event so the stream accounts for every planned unit.
    assert_eq!(stats.cancelled_units, 2);
    let cancelled: Vec<&String> = reporter
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::UnitFinished {
                unit_id,
                status: UnitStatus::Cancelled,
            } => Some(unit_id),
            _ => None,
        })
        .collect();
    assert_eq!(cancelled, vec!["a", "b"]);

    // Teardown completed and aggregated stats were emitted (the run returned
    // rather than hanging on an orphaned worker).
    assert!(
        reporter
            .events()
            .iter()
            .any(|event| matches!(event, Event::RunFinished { .. }))
    );
}

#[test]
fn persistent_ready_gates_dependent_and_tears_down_after_drain() {
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let mut test = unit("test");
    test.depends_on = vec!["service".to_string()];
    let plan = Plan::new(
        vec![service, test],
        vec![vec!["service".into()], vec!["test".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new());
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    let stats = run(&plan, &runner, &cache, &mut reporter);

    assert_eq!(
        runner.started(),
        vec!["service".to_string(), "test".to_string()]
    );
    assert_eq!(runner.shutdowns(), vec!["service".to_string()]);
    assert_eq!(stats.ran_units, 2);
    assert!(
        reporter
            .events()
            .iter()
            .any(|event| { matches!(event, Event::UnitReady { unit_id } if unit_id == "service") })
    );
}

#[test]
fn persistent_readiness_failure_blocks_dependents() {
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let mut test = unit("test");
    test.depends_on = vec!["service".to_string()];
    let plan = Plan::new(
        vec![service, test],
        vec![vec!["service".into()], vec!["test".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_persistent_failure("service"));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    let stats = run(&plan, &runner, &cache, &mut reporter);

    assert_eq!(runner.started(), vec!["service".to_string()]);
    assert_eq!(stats.failed_readiness_units, 1);
    assert_eq!(stats.blocked_units, 1);
}

#[test]
fn failed_dependent_drains_persistent_service_immediately() {
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let mut test = unit("test");
    test.depends_on = vec!["service".to_string()];
    let plan = Plan::new(
        vec![service, test],
        vec![vec!["service".into()], vec!["test".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_failure("test"));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run(&plan, &runner, &cache, &mut reporter);

    let events = reporter.events();
    let test_failed = events
        .iter()
        .position(|event| {
            matches!(event, Event::UnitFinished { unit_id, status: UnitStatus::Failed } if unit_id == "test")
        })
        .expect("test failed event");
    let service_torn_down = events
        .iter()
        .position(|event| {
            matches!(event, Event::UnitFinished { unit_id, status: UnitStatus::TornDown } if unit_id == "service")
        })
        .expect("service torn down event");
    assert!(service_torn_down > test_failed);
    assert_eq!(runner.shutdowns(), vec!["service".to_string()]);
}

#[test]
fn raw_output_routes_normal_as_block_and_persistent_as_live() {
    let normal = unit("normal");
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let mut failed = unit("failed");
    failed.cache_key = Some("failed-key".to_string());
    let mut readiness = unit("readiness");
    readiness.persistent = true;
    readiness.cache = CacheVerdict::Disabled;
    readiness.cache_key = None;
    let plan = Plan::new(
        vec![normal, service, failed, readiness],
        vec![vec![
            "normal".into(),
            "service".into(),
            "failed".into(),
            "readiness".into(),
        ]],
    );
    let runner = Arc::new(
        FakeCommandRunner::new()
            .with_failure("failed")
            .with_persistent_failure("readiness")
            .with_output(
                "normal",
                vec![chunk("normal", OutputStream::Stdout, b"normal\n")],
            )
            .with_output(
                "service",
                vec![chunk("service", OutputStream::Stdout, b"service-live\n")],
            )
            .with_output(
                "failed",
                vec![chunk("failed", OutputStream::Stderr, b"failed\n")],
            )
            .with_output(
                "readiness",
                vec![chunk("readiness", OutputStream::Stderr, b"not-ready\n")],
            ),
    );
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();
    let sink = RecordingRawOutputSink::new();

    run_with_sink(&plan, runner, &cache, &mut reporter, sink.clone());

    let blocks = sink.blocks();
    assert!(blocks.contains(&(
        "normal".to_string(),
        vec![chunk("normal", OutputStream::Stdout, b"normal\n")]
    )));
    assert!(blocks.contains(&(
        "failed".to_string(),
        vec![chunk("failed", OutputStream::Stderr, b"failed\n")]
    )));
    let live = sink.live_chunks();
    assert!(live.contains(&chunk("service", OutputStream::Stdout, b"service-live\n")));
    assert!(live.contains(&chunk("readiness", OutputStream::Stderr, b"not-ready\n")));
}

#[test]
fn cache_writes_only_successful_cacheable_units() {
    let success = unit("success");
    let mut forced = unit("forced");
    forced.cache = CacheVerdict::Forced;
    forced.cache_key = Some("key-forced".to_string());
    let mut disabled = unit("disabled");
    disabled.cache = CacheVerdict::Disabled;
    disabled.cache_key = Some("key-disabled".to_string());
    let mut hit = unit("hit");
    hit.cache = CacheVerdict::Hit;
    hit.cache_key = Some("key-hit".to_string());
    let mut failed = unit("failed");
    failed.cache_key = Some("key-failed".to_string());
    let mut readiness = unit("readiness");
    readiness.persistent = true;
    readiness.cache = CacheVerdict::Disabled;
    readiness.cache_key = Some("key-readiness".to_string());
    let mut blocked = unit("blocked");
    blocked.depends_on = vec!["failed".to_string()];
    blocked.cache_key = Some("key-blocked".to_string());
    let plan = Plan::new(
        vec![success, forced, disabled, hit, failed, readiness, blocked],
        vec![
            vec![
                "success".into(),
                "forced".into(),
                "disabled".into(),
                "hit".into(),
                "failed".into(),
                "readiness".into(),
            ],
            vec!["blocked".into()],
        ],
    );
    let runner = Arc::new(
        FakeCommandRunner::new()
            .with_failure("failed")
            .with_persistent_failure("readiness"),
    );
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run(&plan, &runner, &cache, &mut reporter);

    let mut recorded = cache.recorded();
    recorded.sort();
    assert_eq!(
        recorded,
        vec!["key-forced".to_string(), "key-success".to_string()]
    );
}

#[test]
fn transitive_fail_closed_blocks_chain_but_runs_independent_unit() {
    let a = unit("a");
    let independent = unit("independent");
    let mut b = unit("b");
    b.depends_on = vec!["a".to_string()];
    let mut c = unit("c");
    c.depends_on = vec!["b".to_string()];
    let plan = Plan::new(
        vec![a, independent, b, c],
        vec![
            vec!["a".into(), "independent".into()],
            vec!["b".into()],
            vec!["c".into()],
        ],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_failure("a"));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    let stats = run(&plan, &runner, &cache, &mut reporter);

    assert_eq!(stats.blocked_units, 2);
    assert!(runner.started().contains(&"independent".to_string()));
    assert!(!runner.started().contains(&"b".to_string()));
    assert!(!runner.started().contains(&"c".to_string()));
}

#[test]
fn persistent_service_waits_until_all_dependents_are_terminal() {
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let mut a = unit("a");
    a.depends_on = vec!["service".to_string()];
    let mut b = unit("b");
    b.depends_on = vec!["service".to_string()];
    let plan = Plan::new(
        vec![service, a, b],
        vec![vec!["service".into()], vec!["a".into(), "b".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_failure("a"));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run(&plan, &runner, &cache, &mut reporter);

    let events = reporter.events();
    let b_finished = events
        .iter()
        .position(|event| matches!(event, Event::UnitFinished { unit_id, .. } if unit_id == "b"))
        .expect("b finished");
    let service_torn_down = events
        .iter()
        .position(|event| {
            matches!(event, Event::UnitFinished { unit_id, status: UnitStatus::TornDown } if unit_id == "service")
        })
        .expect("service torn down");
    assert!(service_torn_down > b_finished);
    assert_eq!(runner.shutdowns(), vec!["service".to_string()]);
}

#[test]
fn persistent_goal_without_dependents_is_held_until_run_end_backstop() {
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let normal = unit("normal");
    let plan = Plan::new(
        vec![service, normal],
        vec![vec!["service".into()], vec!["normal".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new());
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run(&plan, &runner, &cache, &mut reporter);

    let events = reporter.events();
    let normal_finished = events
        .iter()
        .position(|event| {
            matches!(event, Event::UnitFinished { unit_id, status: UnitStatus::Succeeded } if unit_id == "normal")
        })
        .expect("normal finished");
    let service_torn_down = events
        .iter()
        .position(|event| {
            matches!(event, Event::UnitFinished { unit_id, status: UnitStatus::TornDown } if unit_id == "service")
        })
        .expect("service torn down");
    assert!(service_torn_down > normal_finished);
}

#[test]
fn lifo_backstop_tears_down_multiple_persistent_goals_in_reverse_start_order() {
    let mut first = unit("first");
    first.persistent = true;
    first.cache = CacheVerdict::Disabled;
    first.cache_key = None;
    let mut second = unit("second");
    second.persistent = true;
    second.cache = CacheVerdict::Disabled;
    second.cache_key = None;
    let plan = Plan::new(
        vec![first, second],
        vec![vec!["first".into()], vec!["second".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new());
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run(&plan, &runner, &cache, &mut reporter);

    assert_eq!(
        runner.shutdowns(),
        vec!["second".to_string(), "first".to_string()]
    );
}

#[test]
fn fail_fast_cancels_persistent_unit_waiting_for_readiness() {
    let a = unit("a");
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let c = unit("c");
    let plan = Plan::new(
        vec![a, service, c],
        vec![vec!["a".into(), "service".into()], vec!["c".into()]],
    );
    let runner = Arc::new(
        FakeCommandRunner::new()
            .with_failure("a")
            .with_blocking_persistent("service"),
    );
    let runner_port: Arc<dyn CommandRunner> = runner.clone();
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();
    let sink = RecordingRawOutputSink::new();
    let mut output = UnitOutputChannel::new(sink);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");

    let stats = runtime
        .block_on(apply(
            &plan,
            runner_port,
            &cache,
            &mut reporter,
            &mut output,
            ApplyOptions {
                max_parallel: 2,
                fail_fast: true,
                ..ApplyOptions::default()
            },
            tokio_util::sync::CancellationToken::new(),
        ))
        .expect("apply succeeds");

    assert!(stats.has_failures());
    assert_eq!(runner.cancelled(), vec!["service".to_string()]);
    assert!(!runner.started().contains(&"c".to_string()));
}

#[test]
fn process_command_runner_smoke_covers_argv_cwd_capture_nonzero_and_readiness() {
    let workspace = TestWorkspace::new("process-command-runner");
    let runner = toven_engine::apply::ProcessCommandRunner::new(workspace.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").expect("PATH available for smoke test"),
    );
    let environment = toven_ports::InvocationEnvironment::explicit(env);

    let ok = toven_ports::Invocation::new(
        "ok",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "pwd; printf err >&2".to_string(),
        ],
    )
    .with_environment(environment.clone());
    let ok_result = runtime
        .block_on(runner.run(&ok, tokio_util::sync::CancellationToken::new()))
        .expect("ok run");
    assert!(ok_result.success);
    let expected_cwd = workspace
        .path()
        .canonicalize()
        .expect("workspace canonicalizes")
        .display()
        .to_string();
    assert!(ok_result.output.iter().any(|chunk| {
        chunk.stream == OutputStream::Stdout
            && String::from_utf8_lossy(&chunk.bytes).trim_end() == expected_cwd
    }));
    assert!(ok_result.output.iter().any(|chunk| {
        chunk.stream == OutputStream::Stderr && String::from_utf8_lossy(&chunk.bytes) == "err"
    }));

    let nonzero = toven_ports::Invocation::new(
        "bad",
        vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()],
    )
    .with_environment(environment.clone());
    let bad_result = runtime
        .block_on(runner.run(&nonzero, tokio_util::sync::CancellationToken::new()))
        .expect("bad run maps to outcome");
    assert!(!bad_result.success);
    assert_eq!(bad_result.exit_code, Some(7));

    let persistent = toven_ports::Invocation::new(
        "srv",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf ready; sleep 1".to_string(),
        ],
    )
    .with_persistent(true)
    .with_readiness(ExecutionReadiness::OutputContains("ready".to_string()))
    .with_readiness_timeout(Duration::from_secs(2))
    .with_environment(environment);
    let live = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observer = {
        let live = std::sync::Arc::clone(&live);
        toven_ports::OutputObserver::new(move |chunk| {
            live.lock().expect("live poisoned").push(chunk);
        })
    };
    let started = runtime
        .block_on(runner.start_persistent(
            &persistent,
            tokio_util::sync::CancellationToken::new(),
            observer,
        ))
        .expect("persistent start");
    match started {
        toven_ports::StartOutcome::Ready { process, .. } => process.shutdown().expect("shutdown"),
        toven_ports::StartOutcome::FailedReadiness { .. } => panic!("readiness should succeed"),
    }
    assert!(
        live.lock()
            .expect("live poisoned")
            .iter()
            .any(|chunk| chunk.bytes == b"ready")
    );
}

#[test]
fn process_command_runner_readiness_command_inherits_invocation_environment() {
    let workspace = TestWorkspace::new("readiness-command-env");
    let runner = toven_engine::apply::ProcessCommandRunner::new(workspace.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").expect("PATH available for readiness command test"),
    );
    let environment = toven_ports::InvocationEnvironment::explicit(env);

    // The readiness probe itself spawns `sh`, which can only be resolved if the
    // probe inherits the invocation's PATH allowlist. With an empty probe
    // environment this readiness command would fail to spawn and report
    // FailedReadiness even though the persistent command runs fine.
    let persistent = toven_ports::Invocation::new(
        "srv",
        vec!["sh".to_string(), "-c".to_string(), "sleep 1".to_string()],
    )
    .with_persistent(true)
    .with_readiness(ExecutionReadiness::Command(vec![
        "sh".to_string(),
        "-c".to_string(),
        "exit 0".to_string(),
    ]))
    .with_readiness_timeout(Duration::from_secs(2))
    .with_environment(environment);

    let observer = toven_ports::OutputObserver::new(|_chunk| {});
    let started = runtime
        .block_on(runner.start_persistent(
            &persistent,
            tokio_util::sync::CancellationToken::new(),
            observer,
        ))
        .expect("persistent start");
    match started {
        toven_ports::StartOutcome::Ready { process, .. } => process.shutdown().expect("shutdown"),
        toven_ports::StartOutcome::FailedReadiness { output } => {
            panic!("readiness command should inherit PATH and succeed, got: {output:?}")
        }
    }
}

#[test]
fn wave_error_still_tears_down_held_persistent_processes() {
    // A persistent service reaches readiness in wave 0 and is held; wave 1 then
    // aborts with a propagated runner error. Teardown and pool shutdown must
    // still run so the held process is not leaked, while the original error is
    // surfaced to the caller.
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let boom = unit("boom");
    let plan = Plan::new(
        vec![service, boom],
        vec![vec!["service".into()], vec!["boom".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_error("boom"));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    let runner_port: Arc<dyn CommandRunner> = runner.clone();
    let mut output = UnitOutputChannel::new(RecordingRawOutputSink::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    let result = runtime.block_on(apply(
        &plan,
        runner_port,
        &cache,
        &mut reporter,
        &mut output,
        ApplyOptions {
            max_parallel: 2,
            fail_fast: false,
            ..ApplyOptions::default()
        },
        tokio_util::sync::CancellationToken::new(),
    ));

    assert!(result.is_err(), "wave error must propagate to the caller");
    assert_eq!(
        runner.shutdowns(),
        vec!["service".to_string()],
        "held persistent process must be torn down despite the wave error"
    );
}

/// Run `apply` with a tiny live-output bridge and an overall timeout so a
/// teardown deadlock surfaces as a test failure (the watchdog aborts the
/// blocked runtime thread) instead of hanging the suite indefinitely.
fn run_backpressured_with_timeout(
    plan: &Plan,
    runner: Arc<FakeCommandRunner>,
    cache: &RecordingCacheWriter,
    reporter: &mut RecordingReporter,
) -> toven_model::RunStats {
    let runner_port: Arc<dyn CommandRunner> = runner;
    let mut output = UnitOutputChannel::new(RecordingRawOutputSink::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    runtime
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(10),
                apply(
                    plan,
                    runner_port,
                    cache,
                    reporter,
                    &mut output,
                    ApplyOptions {
                        max_parallel: 2,
                        fail_fast: false,
                        // One slot: a held process flushing more than one chunk
                        // during teardown parks its reader thread until the
                        // consumer drains, reproducing the deadlock shape.
                        live_output_capacity: 1,
                        ..ApplyOptions::default()
                    },
                    tokio_util::sync::CancellationToken::new(),
                ),
            )
            .await
        })
        .expect("apply must not deadlock during teardown backpressure")
        .expect("apply succeeds")
}

#[test]
fn teardown_drains_live_output_under_backpressure_lifo_backstop() {
    // A persistent goal with no dependents is held until run end and torn down
    // through the LIFO backstop. It flushes more output than the bridge holds
    // during shutdown; teardown must keep draining concurrently or deadlock.
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let normal = unit("normal");
    let plan = Plan::new(
        vec![service, normal],
        vec![vec!["service".into()], vec!["normal".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_teardown_output("service", 8));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run_backpressured_with_timeout(&plan, runner.clone(), &cache, &mut reporter);

    assert_eq!(runner.shutdowns(), vec!["service".to_string()]);
}

#[test]
fn teardown_drains_live_output_under_backpressure_dependent_drain() {
    // A persistent service whose only dependent finishes is torn down mid-run
    // via the per-unit teardown path. It flushes more output than the bridge
    // holds during shutdown; the per-unit teardown must drain concurrently.
    let mut service = unit("service");
    service.persistent = true;
    service.cache = CacheVerdict::Disabled;
    service.cache_key = None;
    let mut test = unit("test");
    test.depends_on = vec!["service".to_string()];
    let plan = Plan::new(
        vec![service, test],
        vec![vec!["service".into()], vec!["test".into()]],
    );
    let runner = Arc::new(FakeCommandRunner::new().with_teardown_output("service", 8));
    let cache = RecordingCacheWriter::new();
    let mut reporter = RecordingReporter::new();

    run_backpressured_with_timeout(&plan, runner.clone(), &cache, &mut reporter);

    assert_eq!(runner.shutdowns(), vec!["service".to_string()]);
}
