//! Shared end-to-end smoke harness for the app binaries (`toven`, `toven-rs`,
//! `toven-go`).
//!
//! Every app's `tests/smoke*.rs` drives the *real* shipping binary against a
//! [`SampleRepo`](crate::repo::SampleRepo) tree, so each app duplicated a tiny
//! `run(cwd, args)` helper. This module is the one shared home for that
//! harness: a [`RunResult`] value plus [`run`]/[`run_ok`] so app tests read as
//! declarative `(argv, expectation)` case lists instead of re-declaring process
//! plumbing.
//!
//! The binary path is passed in by the caller, because
//! `env!("CARGO_BIN_EXE_…")` only expands inside the app crate that owns the
//! binary; the helper therefore stays binary-agnostic and takes the resolved
//! path as an argument.
//!
//! ## Stream routing (why [`RunResult`] carries both streams)
//! The CLI splits its output by purpose: introspection tables (`modules`,
//! `graph`, `affected`, `explain`), `cache path`/`cache stats`, the `init`
//! document, and the `jsonl` event stream go to **stdout**; the human run
//! reporter (`plan`/run summaries, per-unit status), `driver`/`federation`
//! status lines, and `cache clean` diagnostics go to **stderr**. Assertions
//! must target the correct stream, so both are captured verbatim.

use std::collections::BTreeMap;
use std::path::Path;

pub use crate::exec::{CLOCK_EPOCH_ENV, CLOCK_EPOCH_VALUE, program_on_path};

/// The captured result of running an app binary once.
///
/// Holds the full `stdout`/`stderr` (decoded lossily as UTF-8) and the process
/// exit `code` (`None` if the process was terminated by a signal).
#[derive(Debug, Clone)]
pub struct RunResult {
    /// The verbatim argv the binary was invoked with (for assertion messages).
    pub args: Vec<String>,
    /// Everything written to standard output.
    pub stdout: String,
    /// Everything written to standard error.
    pub stderr: String,
    /// The process exit code, or `None` if it was killed by a signal.
    pub code: Option<i32>,
}

impl RunResult {
    /// Whether the process exited successfully (exit code `0`).
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Assert a zero exit, returning `&self` for chaining. Panics with both
    /// streams on a non-zero/killed exit.
    pub fn expect_success(&self) -> &Self {
        assert!(
            self.success(),
            "{}",
            self.diagnostic("expected a zero exit")
        );
        self
    }

    /// Assert a specific non-zero exit code, returning `&self` for chaining.
    pub fn expect_code(&self, code: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(code),
            "{}",
            self.diagnostic(&format!("expected exit code {code}"))
        );
        self
    }

    /// Assert that `stdout` contains `needle`, returning `&self` for chaining.
    pub fn expect_stdout_contains(&self, needle: &str) -> &Self {
        assert!(
            self.stdout.contains(needle),
            "{}",
            self.diagnostic(&format!("expected stdout to contain {needle:?}"))
        );
        self
    }

    /// Assert that `stderr` contains `needle`, returning `&self` for chaining.
    pub fn expect_stderr_contains(&self, needle: &str) -> &Self {
        assert!(
            self.stderr.contains(needle),
            "{}",
            self.diagnostic(&format!("expected stderr to contain {needle:?}"))
        );
        self
    }

    /// Assert that `stdout` equals `expected` exactly, returning `&self` for
    /// chaining.
    ///
    /// The snapshot assertion for the deterministic surfaces: under the pinned
    /// clock the `jsonl` Event stream is byte-stable, so an exact match locks
    /// the whole projection (not just probed substrings) and fails on any
    /// unintended change to the emitted events.
    pub fn expect_stdout_eq(&self, expected: &str) -> &Self {
        assert!(
            self.stdout == expected,
            "{}",
            self.diagnostic(&format!(
                "stdout did not match the snapshot\n  expected:\n{expected}"
            ))
        );
        self
    }

    /// Render a full diagnostic (argv, exit code, both streams) for a failed
    /// assertion.
    fn diagnostic(&self, what: &str) -> String {
        format!(
            "{what}\n  argv: {}\n  code: {:?}\n  stdout:\n{}\n  stderr:\n{}",
            self.args.join(" "),
            self.code,
            self.stdout,
            self.stderr,
        )
    }
}

/// Run `binary <args>` in `cwd`, capturing both streams and the exit code.
///
/// Delegates to the shared [`crate::exec`] spawn path (rskit-process): stdin
/// is closed, and the wall clock is pinned via [`CLOCK_EPOCH_ENV`] so the
/// emitted `run_id` (the only clock-derived field in the Event stream) is
/// deterministic. Panics only when the spawn path itself fails — the binary
/// cannot be run, or a stream exceeded the capture bound (both genuine
/// test-setup failures); a non-zero exit is returned as data so callers can
/// assert on failure paths too.
#[must_use]
pub fn run(binary: &Path, cwd: &Path, args: &[&str]) -> RunResult {
    let owned_args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let env = BTreeMap::from([(CLOCK_EPOCH_ENV.to_owned(), CLOCK_EPOCH_VALUE.to_owned())]);
    let capture = crate::exec::capture(binary, cwd, &owned_args, &env).unwrap_or_else(|error| {
        panic!(
            "failed to run {} in {} with args {args:?}: {error}",
            binary.display(),
            cwd.display(),
        )
    });
    RunResult {
        args: owned_args,
        stdout: capture.stdout,
        stderr: capture.stderr,
        code: capture.code,
    }
}

/// Run `binary <args>` in `cwd` and assert a zero exit, returning the result.
///
/// The common green-path case: `run` followed by [`RunResult::expect_success`].
/// The returned [`RunResult`] is often discarded (the success assertion is the
/// point), so it is intentionally not `#[must_use]`.
pub fn run_ok(binary: &Path, cwd: &Path, args: &[&str]) -> RunResult {
    let result = run(binary, cwd, args);
    result.expect_success();
    result
}
