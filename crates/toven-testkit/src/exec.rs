//! The one shared blocking spawn path for the smoke and scenario harnesses.
//!
//! Both harnesses run a real binary in a working directory and capture its
//! streams and exit code. Process execution is `rskit-process`'s concern
//! ([`rskit_process::run`]); this module only shapes it for the test
//! harnesses: closed stdin, bounded capture, an env overlay, and the pinned
//! clock constants shared by every spawn.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_process::{CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessSpec};

/// Environment variable that pins the CLI's wall clock to a fixed epoch second.
///
/// Mirrors `toven_cli::host::RUN_CLOCK_EPOCH_ENV` (kept as a literal here
/// because the dev-only testkit does not depend on the CLI crate). Every
/// harness spawn sets it so the machine-readable Event stream — whose only
/// wall-clock field is the `run_id` — is byte-for-byte deterministic.
pub const CLOCK_EPOCH_ENV: &str = "TOVEN_CLOCK_EPOCH";

/// The fixed epoch second the harnesses pin the clock to.
///
/// Any stable value works; this one keeps the derived `run_id` (`run-<epoch>`)
/// obvious in snapshots.
pub const CLOCK_EPOCH_VALUE: &str = "1700000000";

/// Per-stream capture bound; deterministic test output far beyond this is a
/// defect worth failing loudly on, not silently truncating.
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

/// The captured result of one binary invocation.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Everything written to standard output (lossy UTF-8).
    pub stdout: String,
    /// Everything written to standard error (lossy UTF-8).
    pub stderr: String,
    /// The process exit code, or `None` if it was killed by a signal.
    pub code: Option<i32>,
}

/// Run `binary <args>` in `cwd` with `env` overlaid on the inherited
/// environment, capturing both streams and the exit code. Stdin is closed.
///
/// # Errors
///
/// Returns a typed [`AppError`] when the binary cannot be spawned or a stream
/// exceeds the capture bound (never a silent truncation).
pub fn capture(
    binary: &Path,
    cwd: &Path,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> AppResult<Capture> {
    let mut spec = ProcessSpec::new(binary);
    spec.args = args.iter().map(OsString::from).collect();
    spec.dir = Some(cwd.to_path_buf());
    spec.env = env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let io = CapturedIo::new()
        .with_output(OutputPolicy::captured().with_max_output_bytes(MAX_CAPTURE_BYTES));
    let config = ProcessConfig::default().with_io(ProcessIo::captured(io));
    let result = rskit_process::run(&spec, &config)?;
    if result.stdout_truncated || result.stderr_truncated {
        return Err(AppError::conflict(format!(
            "captured output of {} exceeded the {MAX_CAPTURE_BYTES}-byte bound",
            binary.display()
        )));
    }
    Ok(Capture {
        stdout: result.stdout,
        stderr: result.stderr,
        code: result.exit_code,
    })
}

/// Whether `program` is discoverable as an executable on `PATH`.
///
/// Used to gate toolchain-dependent scenarios and smokes (e.g. skip a `go`
/// APPLY when no `go` toolchain is installed) so a runner without that
/// toolchain stays green instead of failing.
///
/// The probe uses Unix executable-bit semantics (via `rskit_fs`), matching
/// Toven's currently Unix-only runtime stack (`rskit-process` does not yet
/// build on Windows). A cross-platform PATH lookup — honouring Windows
/// `PATHEXT` / `.exe` — belongs in a future generic `which`-style helper in
/// rskit rather than a bespoke branch here, and lands with the tracked Windows
/// port.
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
