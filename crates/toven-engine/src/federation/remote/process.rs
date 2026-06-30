//! Subprocess spawning and a per-RPC kill watchdog for the driver transport.
//!
//! A driver is launched **argv-only** (`<program> __serve`) with piped
//! stdin/stdout for framed RPC and inherited stderr for its own diagnostics —
//! the driver boundary carries no child build output. stdin/stdout pipes are the
//! transport; the umbrella never passes a shell string.
//!
//! The transport keeps an interactive bidirectional pipe open across many
//! request/response frames while staying argv-only and reusing rskit's error
//! vocabulary.

use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::super::protocol::handshake::DriverFault;

/// Lock the shared child, recovering the guard if a prior holder panicked.
///
/// Teardown must never itself panic on a poisoned lock; recovering the inner
/// guard lets kill/reap proceed.
fn lock_child(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A spawned driver subprocess plus its transport pipes.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct SpawnedDriver {
    /// Child handle, shared with the kill watchdog.
    pub(crate) child: Arc<Mutex<Child>>,
    /// The driver's stdin — the umbrella writes request frames here.
    pub(crate) stdin: ChildStdin,
    /// The driver's stdout — the umbrella reads response frames here.
    pub(crate) stdout: ChildStdout,
}

/// Spawn `program <subcommand>`, wiring piped stdin/stdout and inherited stderr.
///
/// `subcommand` is the hidden driver entry to launch — `__serve` for the
/// port-call protocol or `__scaffold` for the config-less scaffold exchange.
///
/// # Errors
/// Returns [`DriverFault::Spawn`] if the process cannot be launched or its
/// stdin/stdout pipes cannot be captured.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn spawn(
    program: &std::path::Path,
    subcommand: &str,
) -> Result<SpawnedDriver, DriverFault> {
    let mut child = Command::new(program)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| DriverFault::Spawn(format!("{}: {error}", program.display())))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| DriverFault::Spawn("driver stdin pipe was not captured".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DriverFault::Spawn("driver stdout pipe was not captured".to_string()))?;

    Ok(SpawnedDriver {
        child: Arc::new(Mutex::new(child)),
        stdin,
        stdout,
    })
}

/// A handle that terminates and reaps the driver child on drop.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct ChildHandle {
    child: Arc<Mutex<Child>>,
}

impl ChildHandle {
    /// Wrap a shared child for teardown.
    pub(crate) const fn new(child: Arc<Mutex<Child>>) -> Self {
        Self { child }
    }

    /// Arm a watchdog that kills the child if it is not disarmed within `timeout`.
    pub(crate) fn arm_watchdog(&self, timeout: Duration) -> Watchdog {
        Watchdog::arm(Arc::clone(&self.child), timeout)
    }
}

impl Drop for ChildHandle {
    fn drop(&mut self) {
        // The umbrella already dropped the stdin pipe (driver sees EOF and
        // exits); kill + reap guards against a wedged child and zombies.
        let mut child = lock_child(&self.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A single-shot timer thread that kills the driver child on deadline.
///
/// Armed before a blocking RPC read and disarmed once the read completes. If the
/// deadline elapses first, the child is killed so the blocked read unblocks with
/// a transport error; [`Watchdog::disarm`] then reports that the timeout fired so
/// the caller can classify it as [`DriverFault::Timeout`].
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct Watchdog {
    disarm: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
    fired: Arc<std::sync::atomic::AtomicBool>,
}

impl Watchdog {
    /// Spawn the watchdog timer for `timeout`.
    fn arm(child: Arc<Mutex<Child>>, timeout: Duration) -> Self {
        let (disarm, rx) = mpsc::channel();
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired_timer = Arc::clone(&fired);
        let handle = thread::spawn(move || {
            // Only a true timeout fires the kill; an explicit disarm (Ok) or a
            // dropped sender after disarm (Disconnected) must not.
            if rx.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
                fired_timer.store(true, std::sync::atomic::Ordering::SeqCst);
                let mut child = lock_child(&child);
                let _ = child.kill();
            }
        });
        Self {
            disarm,
            handle: Some(handle),
            fired,
        }
    }

    /// Disarm the watchdog, join its thread, and report whether it fired.
    pub(crate) fn disarm(mut self) -> bool {
        let _ = self.disarm.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.fired.load(std::sync::atomic::Ordering::SeqCst)
    }
}
