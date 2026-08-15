//! Scriptable [`CommandRunner`] double for APPLY wave-walk tests.
//!
//! [`FakeCommandRunner`] runs no real subprocess: each unit's outcome is
//! scripted (success / failure / readiness failure / cancel-aware blocking) and
//! the runner records start order, peak concurrency, cancellations, and
//! persistent-teardown (shutdown) order so tests can assert scheduling behavior
//! deterministically.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_process::LifecyclePolicy;
use tokio_util::sync::CancellationToken;
use toven_model::{OutputStream, UnitOutput};
use toven_ports::{
    CommandRunner, HeldProcess, Invocation, OutputObserver, RunOutcome, StartOutcome,
};

/// How long a non-blocking scripted run "occupies" a slot, so concurrent runs
/// overlap observably and peak concurrency reflects real parallelism.
const RUN_WINDOW: Duration = Duration::from_millis(15);

#[derive(Default)]
struct RunLog {
    started: Vec<String>,
    finished: Vec<String>,
    cancelled: Vec<String>,
    coactive: Vec<(String, Vec<String>)>,
    lifecycles: Vec<(String, LifecyclePolicy)>,
}

/// A scriptable [`CommandRunner`] that records scheduling behavior.
pub struct FakeCommandRunner {
    failures: HashSet<String>,
    errors: HashSet<String>,
    persistent_failures: HashSet<String>,
    blocking: HashSet<String>,
    blocking_persistent: HashSet<String>,
    outputs: HashMap<String, Vec<UnitOutput>>,
    teardown_outputs: HashMap<String, usize>,
    log: Arc<Mutex<RunLog>>,
    shutdowns: Arc<Mutex<Vec<String>>>,
    active: Arc<Mutex<HashSet<String>>>,
    peak: AtomicUsize,
}

impl Default for FakeCommandRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeCommandRunner {
    /// A runner where every unit succeeds immediately with no output.
    #[must_use]
    pub fn new() -> Self {
        Self {
            failures: HashSet::new(),
            errors: HashSet::new(),
            persistent_failures: HashSet::new(),
            blocking: HashSet::new(),
            blocking_persistent: HashSet::new(),
            outputs: HashMap::new(),
            teardown_outputs: HashMap::new(),
            log: Arc::new(Mutex::new(RunLog::default())),
            shutdowns: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(Mutex::new(HashSet::new())),
            peak: AtomicUsize::new(0),
        }
    }

    /// Script `unit_id`'s normal run to fail (non-zero exit).
    #[must_use]
    pub fn with_failure(mut self, unit_id: impl Into<String>) -> Self {
        self.failures.insert(unit_id.into());
        self
    }

    /// Script `unit_id`'s normal run to return a propagated runner error
    /// (`Err`), as opposed to a non-zero exit. Models spawn/IO failures that
    /// abort the run rather than recording a unit failure.
    #[must_use]
    pub fn with_error(mut self, unit_id: impl Into<String>) -> Self {
        self.errors.insert(unit_id.into());
        self
    }

    /// Script `unit_id`'s persistent start to fail readiness.
    #[must_use]
    pub fn with_persistent_failure(mut self, unit_id: impl Into<String>) -> Self {
        self.persistent_failures.insert(unit_id.into());
        self
    }

    /// Make `unit_id` block until cancelled (to exercise `--fail-fast`).
    #[must_use]
    pub fn with_blocking(mut self, unit_id: impl Into<String>) -> Self {
        self.blocking.insert(unit_id.into());
        self
    }

    /// Make a persistent start block until cancelled.
    #[must_use]
    pub fn with_blocking_persistent(mut self, unit_id: impl Into<String>) -> Self {
        self.blocking_persistent.insert(unit_id.into());
        self
    }

    /// Script raw output chunks `unit_id` emits.
    #[must_use]
    pub fn with_output(mut self, unit_id: impl Into<String>, output: Vec<UnitOutput>) -> Self {
        self.outputs.insert(unit_id.into(), output);
        self
    }

    /// Make a persistent `unit_id` emit `count` raw output chunks *during
    /// teardown* on a dedicated reader thread that applies real backpressure
    /// (`blocking_send`) against the bounded live-output bridge.
    ///
    /// With a small `live_output_capacity` this fills the bridge while the
    /// process is shutting down, so the reader thread parks until the APPLY
    /// consumer drains it. It reproduces the production teardown-deadlock
    /// shape: shutdown only completes once the consumer keeps draining live
    /// output concurrently with the (off-thread) blocking shutdown.
    #[must_use]
    pub fn with_teardown_output(mut self, unit_id: impl Into<String>, count: usize) -> Self {
        self.teardown_outputs.insert(unit_id.into(), count);
        self
    }

    /// Unit ids in the order their execution started.
    #[must_use]
    pub fn started(&self) -> Vec<String> {
        self.log.lock().expect("log poisoned").started.clone()
    }

    /// Unit ids in the order their normal execution finished.
    #[must_use]
    pub fn finished(&self) -> Vec<String> {
        self.log.lock().expect("log poisoned").finished.clone()
    }

    /// Unit ids that observed cancellation while blocking.
    #[must_use]
    pub fn cancelled(&self) -> Vec<String> {
        self.log.lock().expect("log poisoned").cancelled.clone()
    }

    /// Persistent unit ids in teardown (shutdown) order.
    #[must_use]
    pub fn shutdowns(&self) -> Vec<String> {
        self.shutdowns.lock().expect("shutdowns poisoned").clone()
    }

    /// The peak number of normal runs active at once.
    #[must_use]
    pub fn peak_concurrency(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    /// For each unit, the other units active at the moment it started.
    #[must_use]
    pub fn coactive(&self) -> Vec<(String, Vec<String>)> {
        self.log.lock().expect("log poisoned").coactive.clone()
    }

    /// For each unit run through the port, the [`LifecyclePolicy`] its
    /// [`Invocation`] carried, in call order. Lets a test assert the caller's
    /// lifecycle intent rides intact across the [`CommandRunner`] boundary.
    #[must_use]
    pub fn lifecycles(&self) -> Vec<(String, LifecyclePolicy)> {
        self.log.lock().expect("log poisoned").lifecycles.clone()
    }

    fn record_lifecycle(&self, unit_id: &str, lifecycle: LifecyclePolicy) {
        self.log
            .lock()
            .expect("log poisoned")
            .lifecycles
            .push((unit_id.to_string(), lifecycle));
    }

    fn enter(&self, unit_id: &str) {
        let mut active = self.active.lock().expect("active poisoned");
        let others: Vec<String> = active.iter().cloned().collect();
        active.insert(unit_id.to_string());
        let now = active.len();
        drop(active);
        self.peak.fetch_max(now, Ordering::SeqCst);
        let mut log = self.log.lock().expect("log poisoned");
        log.started.push(unit_id.to_string());
        log.coactive.push((unit_id.to_string(), others));
    }

    fn leave(&self, unit_id: &str) {
        self.active.lock().expect("active poisoned").remove(unit_id);
    }

    fn output_for(&self, unit_id: &str) -> Vec<UnitOutput> {
        self.outputs.get(unit_id).cloned().unwrap_or_default()
    }
}

/// A held-process double that records its teardown into the shared log.
struct FakeHeldProcess {
    unit_id: String,
    shutdowns: Arc<Mutex<Vec<String>>>,
    /// Live-output sink used to emit teardown chunks under backpressure.
    observer: OutputObserver,
    /// Number of chunks emitted during shutdown on a dedicated reader thread.
    teardown_chunks: usize,
}

impl HeldProcess for FakeHeldProcess {
    fn unit_id(&self) -> &str {
        &self.unit_id
    }

    fn shutdown(self: Box<Self>) -> AppResult<()> {
        // Mirror a real persistent process: a dedicated reader thread flushes final
        // output (here via the live-output observer's `blocking_send` backpressure
        // path) and shutdown joins it. If the APPLY consumer does not keep draining the
        // bounded bridge concurrently with this blocking shutdown, the reader thread
        // parks forever and teardown deadlocks.
        if self.teardown_chunks > 0 {
            let observer = self.observer.clone();
            let unit_id = self.unit_id.clone();
            let count = self.teardown_chunks;
            let reader = std::thread::spawn(move || {
                for index in 0..count {
                    observer.emit(UnitOutput {
                        unit_id: unit_id.clone(),
                        stream: OutputStream::Stdout,
                        bytes: format!("teardown-{index}").into_bytes(),
                    });
                }
            });
            reader.join().map_err(|_| {
                AppError::new(ErrorCode::Internal, "teardown reader thread panicked")
            })?;
        }
        self.shutdowns
            .lock()
            .expect("shutdowns poisoned")
            .push(self.unit_id.clone());
        Ok(())
    }
}

#[async_trait]
impl CommandRunner for FakeCommandRunner {
    async fn run(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
        live: Option<OutputObserver>,
    ) -> AppResult<RunOutcome> {
        let unit_id = invocation.unit_id.clone();
        self.record_lifecycle(&unit_id, invocation.lifecycle);
        self.enter(&unit_id);
        let output = self.output_for(&unit_id);

        if self.errors.contains(&unit_id) {
            self.leave(&unit_id);
            return Err(AppError::new(ErrorCode::Internal, "scripted runner error"));
        }

        if self.blocking.contains(&unit_id) {
            tokio::select! {
                () = cancel.cancelled() => {
                    self.log.lock().expect("log poisoned").cancelled.push(unit_id.clone());
                    self.leave(&unit_id);
                    return Err(AppError::cancelled("normal run"));
                }
                () = tokio::time::sleep(Duration::from_secs(10)) => {}
            }
        } else {
            tokio::time::sleep(RUN_WINDOW).await;
        }

        self.leave(&unit_id);
        self.log
            .lock()
            .expect("log poisoned")
            .finished
            .push(unit_id.clone());
        // Streaming mode mirrors the real runner: emit each chunk live through the
        // observer and return an empty outcome rather than buffered chunks.
        let returned = if let Some(observer) = live {
            for chunk in output {
                observer.emit(chunk);
            }
            Vec::new()
        } else {
            output
        };
        if self.failures.contains(&unit_id) {
            Ok(RunOutcome::failed(Some(1), returned))
        } else {
            Ok(RunOutcome::succeeded(returned))
        }
    }

    async fn start_persistent(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
        output: OutputObserver,
    ) -> AppResult<StartOutcome> {
        let unit_id = invocation.unit_id.clone();
        self.record_lifecycle(&unit_id, invocation.lifecycle);
        self.log
            .lock()
            .expect("log poisoned")
            .started
            .push(unit_id.clone());
        let chunks = self.output_for(&unit_id);
        if self.blocking_persistent.contains(&unit_id) {
            cancel.cancelled().await;
            self.log
                .lock()
                .expect("log poisoned")
                .cancelled
                .push(unit_id.clone());
            return Err(AppError::cancelled("persistent start"));
        }
        if self.persistent_failures.contains(&unit_id) {
            return Ok(StartOutcome::FailedReadiness { output: chunks });
        }
        for chunk in chunks {
            output.emit(chunk);
        }
        let teardown_chunks = self
            .teardown_outputs
            .get(&unit_id)
            .copied()
            .unwrap_or_default();
        Ok(StartOutcome::Ready {
            output: Vec::new(),
            process: Box::new(FakeHeldProcess {
                unit_id,
                shutdowns: Arc::clone(&self.shutdowns),
                observer: output,
                teardown_chunks,
            }),
        })
    }
}
