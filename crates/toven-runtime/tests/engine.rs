//! The generic engine, exercised by a fake unit-operation: streaming, wave
//! ordering, fail-closed gating, bounded concurrency, the once-only GATHER, and
//! early-abort cancellation.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;

use toven_runtime::{
    Completed, EngineConfig, Progress, UnitOperation, UnitReport, UnitSpec, UnitStatus, execute,
};

/// A fake operation: records how often GATHER runs and the peak concurrency,
/// threads the gathered value into each outcome, and fails a named set of units.
struct FakeOp {
    gather_calls: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak_active: Arc<AtomicUsize>,
    fail: HashSet<String>,
    delay: Duration,
}

impl FakeOp {
    fn new(fail: &[&str], delay: Duration) -> Self {
        Self {
            gather_calls: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            peak_active: Arc::new(AtomicUsize::new(0)),
            fail: fail.iter().map(|s| (*s).to_string()).collect(),
            delay,
        }
    }
}

#[async_trait]
impl UnitOperation for FakeOp {
    type Shared = String;
    type Outcome = String;

    async fn gather(&self) -> AppResult<String> {
        self.gather_calls.fetch_add(1, Ordering::SeqCst);
        Ok("shared".to_string())
    }

    async fn run(
        &self,
        shared: &String,
        unit_id: &str,
        _cancel: CancellationToken,
    ) -> AppResult<Completed<String>> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_active.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        let outcome = format!("{unit_id}:{shared}");
        if self.fail.contains(unit_id) {
            Ok(Completed::failed(outcome))
        } else {
            Ok(Completed::succeeded(outcome))
        }
    }
}

/// Records the streamed lifecycle in arrival order.
#[derive(Default)]
struct Recorder {
    started: Vec<String>,
    settled: Vec<(String, UnitStatus, Option<String>)>,
}

impl Recorder {
    fn settled_index(&self, unit_id: &str) -> usize {
        self.settled
            .iter()
            .position(|(id, ..)| id == unit_id)
            .expect("unit settled")
    }

    fn status(&self, unit_id: &str) -> UnitStatus {
        self.settled
            .iter()
            .find(|(id, ..)| id == unit_id)
            .map(|(_, status, _)| *status)
            .expect("unit settled")
    }

    fn outcome(&self, unit_id: &str) -> Option<String> {
        self.settled
            .iter()
            .find(|(id, ..)| id == unit_id)
            .and_then(|(_, _, outcome)| outcome.clone())
    }
}

impl Progress<String> for Recorder {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        self.started.push(unit_id.to_string());
        Ok(())
    }

    fn settled(&mut self, report: &UnitReport<String>) -> AppResult<()> {
        self.settled.push((
            report.unit_id.clone(),
            report.status,
            report.outcome.clone(),
        ));
        Ok(())
    }
}

fn spec(id: &str, deps: &[&str]) -> UnitSpec {
    UnitSpec::new(id, deps.iter().copied())
}

const fn config(jobs: usize, fail_fast: bool) -> EngineConfig {
    EngineConfig { jobs, fail_fast }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edgeless_units_stream_one_parallel_wave() {
    let units = [
        spec("a", &[]),
        spec("b", &[]),
        spec("c", &[]),
        spec("d", &[]),
    ];
    let op = FakeOp::new(&[], Duration::from_millis(40));
    let gather_calls = Arc::clone(&op.gather_calls);
    let peak = Arc::clone(&op.peak_active);
    let mut rec = Recorder::default();

    let summary = execute(
        &units,
        op,
        config(4, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(gather_calls.load(Ordering::SeqCst), 1, "GATHER runs once");
    assert_eq!(rec.started.len(), 4);
    assert_eq!(rec.settled.len(), 4);
    assert!(
        rec.settled
            .iter()
            .all(|(_, s, _)| *s == UnitStatus::Succeeded)
    );
    // The typed shared value is threaded into every per-unit outcome.
    assert_eq!(rec.status("a"), UnitStatus::Succeeded);
    assert_eq!(rec.outcome("a"), Some("a:shared".to_string()));
    assert!(peak.load(Ordering::SeqCst) > 1, "ran concurrently");
    assert_eq!(summary.total, 4);
    assert_eq!(summary.succeeded, 4);
    assert!(!summary.has_failures());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edged_units_settle_in_dependency_order() {
    let units = [
        spec("leaf", &["mid"]),
        spec("mid", &["root"]),
        spec("root", &[]),
        spec("sibling", &["root"]),
    ];
    let op = FakeOp::new(&[], Duration::from_millis(10));
    let mut rec = Recorder::default();

    execute(
        &units,
        op,
        config(4, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert!(rec.settled_index("root") < rec.settled_index("mid"));
    assert!(rec.settled_index("mid") < rec.settled_index("leaf"));
    assert!(rec.settled_index("root") < rec.settled_index("sibling"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failure_blocks_only_transitive_dependents() {
    // root <- {a, b}; c <- a. Failing `a` blocks `c`, leaves `b` and `root` alone.
    let units = [
        spec("root", &[]),
        spec("a", &["root"]),
        spec("b", &["root"]),
        spec("c", &["a"]),
    ];
    let op = FakeOp::new(&["a"], Duration::from_millis(5));
    let mut rec = Recorder::default();

    let summary = execute(
        &units,
        op,
        config(4, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(rec.status("root"), UnitStatus::Succeeded);
    assert_eq!(rec.status("a"), UnitStatus::Failed);
    assert_eq!(rec.status("b"), UnitStatus::Succeeded);
    assert_eq!(rec.status("c"), UnitStatus::Blocked);
    // A blocked unit never ran, so it carries no outcome.
    assert_eq!(rec.outcome("c"), None);
    assert_eq!(summary.succeeded, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.blocked, 1);
    assert!(summary.has_failures());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrency_is_bounded_by_jobs() {
    let units: Vec<UnitSpec> = (0..6).map(|i| spec(&format!("u{i}"), &[])).collect();
    let op = FakeOp::new(&[], Duration::from_millis(40));
    let peak = Arc::clone(&op.peak_active);
    let mut rec = Recorder::default();

    execute(
        &units,
        op,
        config(2, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert!(peak.load(Ordering::SeqCst) <= 2, "never exceeded --jobs");
    assert_eq!(rec.settled.len(), 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gather_runs_once_regardless_of_unit_count() {
    let units: Vec<UnitSpec> = (0..12).map(|i| spec(&format!("u{i}"), &[])).collect();
    let op = FakeOp::new(&[], Duration::from_millis(1));
    let gather_calls = Arc::clone(&op.gather_calls);
    let mut rec = Recorder::default();

    execute(
        &units,
        op,
        config(4, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(gather_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fail_fast_cancels_never_launched_units() {
    // Wave 0: `boom` (fails) and `ok`. Wave 1: `late` depends on `ok` (not on boom),
    // so it is not blocked — it is cancelled because fail-fast stops launching wave 1.
    let units = [spec("boom", &[]), spec("ok", &[]), spec("late", &["ok"])];
    let op = FakeOp::new(&["boom"], Duration::from_millis(5));
    let mut rec = Recorder::default();

    let summary = execute(
        &units,
        op,
        config(4, true),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(rec.status("boom"), UnitStatus::Failed);
    assert_eq!(rec.status("late"), UnitStatus::Cancelled);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.cancelled, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_cancelled_run_settles_every_unit_cancelled() {
    let units = [spec("a", &[]), spec("b", &["a"])];
    let op = FakeOp::new(&[], Duration::from_millis(5));
    let mut rec = Recorder::default();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let summary = execute(&units, op, config(4, false), &mut rec, cancel)
        .await
        .unwrap();

    assert!(rec.started.is_empty(), "nothing launched");
    assert_eq!(summary.cancelled, 2);
    assert_eq!(rec.status("a"), UnitStatus::Cancelled);
    assert_eq!(rec.status("b"), UnitStatus::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_graph_is_rejected_before_gather() {
    let units = [spec("a", &["b"]), spec("b", &["a"])];
    let op = FakeOp::new(&[], Duration::from_millis(1));
    let gather_calls = Arc::clone(&op.gather_calls);
    let mut rec = Recorder::default();

    let err = execute(
        &units,
        op,
        config(2, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("cycle"));
    assert_eq!(
        gather_calls.load(Ordering::SeqCst),
        0,
        "no GATHER on a bad graph"
    );
}

/// A fake operation where one unit returns a hard `Err` and every other unit
/// races a long sleep against its cancel token, recording whether it was cancelled.
struct HardErrOp {
    boom: String,
    cancelled_siblings: Arc<AtomicUsize>,
}

#[async_trait]
impl UnitOperation for HardErrOp {
    type Shared = String;
    type Outcome = String;

    async fn gather(&self) -> AppResult<String> {
        Ok("shared".to_string())
    }

    async fn run(
        &self,
        _shared: &String,
        unit_id: &str,
        cancel: CancellationToken,
    ) -> AppResult<Completed<String>> {
        if unit_id == self.boom {
            // Let the siblings reach their await first, then fail hard.
            tokio::time::sleep(Duration::from_millis(10)).await;
            return Err(rskit_errors::AppError::internal(std::io::Error::other(
                "boom",
            )));
        }
        tokio::select! {
            () = cancel.cancelled() => {
                self.cancelled_siblings.fetch_add(1, Ordering::SeqCst);
                Ok(Completed::succeeded(unit_id.to_string()))
            }
            () = tokio::time::sleep(Duration::from_secs(30)) => {
                Ok(Completed::succeeded(unit_id.to_string()))
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hard_error_cancels_inflight_siblings() {
    // All edgeless, so they run in one wave. `boom` errors hard; the three siblings
    // would otherwise sleep 30s — a prompt return proves they were cancelled.
    let units = [
        spec("boom", &[]),
        spec("s1", &[]),
        spec("s2", &[]),
        spec("s3", &[]),
    ];
    let cancelled_siblings = Arc::new(AtomicUsize::new(0));
    let op = HardErrOp {
        boom: "boom".to_string(),
        cancelled_siblings: Arc::clone(&cancelled_siblings),
    };
    let mut rec = Recorder::default();

    let start = std::time::Instant::now();
    execute(
        &units,
        op,
        config(4, false),
        &mut rec,
        CancellationToken::new(),
    )
    .await
    .expect_err("hard operation error propagates");

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "hard error tore down siblings promptly instead of waiting them out"
    );
    assert_eq!(
        cancelled_siblings.load(Ordering::SeqCst),
        3,
        "every in-flight sibling observed cancellation"
    );
}
