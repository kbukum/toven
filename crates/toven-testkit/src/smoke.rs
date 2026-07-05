//! Shared end-to-end smoke harness for the app binaries (`toven`, `toven-rs`,
//! `toven-go`).
//!
//! Every app's `tests/smoke*.rs` drives the *real* shipping binary against a
//! [`SampleRepo`](crate::repo::SampleRepo) tree, so each app duplicated a tiny
//! `run(cwd, args)` helper. This module is the one shared home for that harness:
//! a [`RunResult`] value plus [`run`]/[`run_ok`] so app tests read as
//! declarative `(argv, expectation)` case lists instead of re-declaring process
//! plumbing.
//!
//! The binary path is passed in by the caller, because `env!("CARGO_BIN_EXE_…")`
//! only expands inside the app crate that owns the binary; the helper therefore
//! stays binary-agnostic and takes the resolved path as an argument.
//!
//! ## Stream routing (why [`RunResult`] carries both streams)
//! The CLI splits its output by purpose: introspection tables (`modules`,
//! `graph`, `affected`, `explain`), `cache path`/`cache stats`, the `generate`
//! document, and the `jsonl` event stream go to **stdout**; the human run
//! reporter (`plan`/run summaries, per-unit status), `driver`/`federation`
//! status lines, and `cache clean` diagnostics go to **stderr**. Assertions must
//! target the correct stream, so both are captured verbatim.

use std::path::Path;
use std::process::{Command, Stdio};

/// Environment variable that pins the CLI's wall clock to a fixed epoch second.
///
/// Mirrors `toven_cli::host::RUN_CLOCK_EPOCH_ENV` (kept as a literal here because
/// the dev-only testkit does not depend on the CLI crate). The shared [`run`]
/// harness sets it on every spawned binary so the machine-readable Event stream
/// — whose only wall-clock field is the `run_id` — is byte-for-byte
/// deterministic, which is what makes snapshotting the `jsonl` projection sound.
/// The demonstrative jsonl snapshot smokes fail loudly if this drifts from the
/// CLI-side constant.
pub const CLOCK_EPOCH_ENV: &str = "TOVEN_CLOCK_EPOCH";

/// The fixed epoch second the harness pins the clock to.
///
/// Any stable value works; this one keeps the derived `run_id` (`run-<epoch>`)
/// obvious in snapshots.
pub const CLOCK_EPOCH_VALUE: &str = "1700000000";

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
/// The process inherits no stdin. Panics only if the binary cannot be spawned
/// at all (a genuine test-setup failure); a non-zero exit is returned as data so
/// callers can assert on failure paths too.
#[must_use]
pub fn run(binary: &Path, cwd: &Path, args: &[&str]) -> RunResult {
    let output = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        // Detach stdin explicitly so any tool that reads it sees EOF instead of
        // blocking on the test runner's stdin; keeps the "inherits no stdin"
        // guarantee true even if this ever moves off `Command::output()`.
        .stdin(Stdio::null())
        // Pin the wall clock so the emitted `run_id` (the only clock-derived
        // field in the Event stream) is deterministic; see `CLOCK_EPOCH_ENV`.
        .env(CLOCK_EPOCH_ENV, CLOCK_EPOCH_VALUE)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to spawn {} in {} with args {args:?}: {error}",
                binary.display(),
                cwd.display(),
            )
        });
    RunResult {
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    }
}

/// Run `binary <args>` in `cwd` and assert a zero exit, returning the result.
///
/// The common green-path case: `run` followed by
/// [`RunResult::expect_success`]. The returned [`RunResult`] is often discarded
/// (the success assertion is the point), so it is intentionally not `#[must_use]`.
pub fn run_ok(binary: &Path, cwd: &Path, args: &[&str]) -> RunResult {
    let result = run(binary, cwd, args);
    result.expect_success();
    result
}

/// Whether `program` is discoverable as an executable on `PATH`.
///
/// Used to gate toolchain-dependent APPLY smokes (e.g. skip the `go` APPLY when
/// no `go` toolchain is installed) so a runner without that toolchain stays
/// green instead of failing.
///
/// The probe uses Unix executable-bit semantics (via `rskit_fs`), matching
/// Toven's currently Unix-only runtime stack (`rskit-process` does not yet build
/// on Windows). A cross-platform PATH lookup — honouring Windows `PATHEXT` /
/// `.exe` — belongs in a future generic `which`-style helper in rskit rather
/// than a bespoke branch here, and lands with the tracked Windows port.
#[must_use]
pub fn program_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        rskit_fs::sync_io::file::is_executable(&candidate).unwrap_or(false)
    })
}
