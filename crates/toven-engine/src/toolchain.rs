//! The process-backed toolchain-probe adapter.
//!
//! Toolchain probing is a subprocess side effect, so the planner injects the
//! [`ToolchainProber`] port and this consuming crate owns the production
//! [`ProcessToolchainProber`].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use toven_ports::{ToolInvocation, ToolRunner, ToolchainProbe, ToolchainProber};

/// Default per-probe timeout: bounded so a hung toolchain cannot stall PLAN.
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on captured probe output (64 KiB) — a version line is tiny.
const MAX_PROBE_OUTPUT_BYTES: usize = 64 * 1024;

/// The production [`ToolchainProber`]: a captured, bounded, timed subprocess.
#[derive(Clone)]
pub struct ProcessToolchainProber {
    runner: Arc<dyn ToolRunner>,
    timeout: Duration,
    max_output_bytes: usize,
}

impl ProcessToolchainProber {
    /// Construct a prober over the injected one-shot tool runner.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self {
            runner,
            timeout: DEFAULT_PROBE_TIMEOUT,
            max_output_bytes: MAX_PROBE_OUTPUT_BYTES,
        }
    }

    /// Test-only prober with a tiny output cap so a stream overrun is
    /// exercisable deterministically without emitting cap-sized output.
    #[cfg(test)]
    fn with_max_output_bytes(runner: Arc<dyn ToolRunner>, max_output_bytes: usize) -> Self {
        Self {
            runner,
            timeout: DEFAULT_PROBE_TIMEOUT,
            max_output_bytes,
        }
    }
}

impl ToolchainProber for ProcessToolchainProber {
    fn probe(&self, probe: &ToolchainProbe, workspace_root: &Path) -> AppResult<String> {
        let argv = std::iter::once(probe.program.clone())
            .chain(probe.args.iter().cloned())
            .collect();
        let invocation = ToolInvocation::new(argv)
            .with_working_dir(workspace_root)
            .with_timeout(self.timeout)
            .with_max_output_bytes(self.max_output_bytes);
        let outcome = self.runner.run(&invocation).map_err(|error| {
            if error.code() == rskit_errors::ErrorCode::NotFound {
                AppError::new(
                    rskit_errors::ErrorCode::NotFound,
                    format!(
                        "toolchain probe '{}' could not run '{}' in '{}': is '{}' installed and on PATH?",
                        probe.label,
                        probe.program,
                        workspace_root.display(),
                        probe.program,
                    ),
                )
                .with_cause(error)
            } else {
                error
            }
        })?;
        if outcome.timed_out {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Timeout,
                format!(
                    "toolchain probe '{}' timed out after {:?} in '{}'",
                    probe.label,
                    self.timeout,
                    workspace_root.display()
                ),
            ));
        }
        if outcome.truncated.any() {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "toolchain probe '{}' output in '{}' exceeded {} bytes",
                    probe.label,
                    workspace_root.display(),
                    self.max_output_bytes
                ),
            ));
        }
        if outcome.succeeded() {
            Ok(outcome.stdout.trim().to_string())
        } else {
            Ok(String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use toven_exec::ProcessToolRunner;
    use toven_ports::{ToolchainProbe, ToolchainProber};

    use super::ProcessToolchainProber;

    fn prober() -> ProcessToolchainProber {
        ProcessToolchainProber::new(Arc::new(ProcessToolRunner::new()))
    }

    #[test]
    fn a_missing_probe_tool_is_a_typed_not_found_error_naming_the_program() {
        let probe = ToolchainProbe {
            label: "structure".to_string(),
            program: "toven-nonexistent-probe-tool".to_string(),
            args: vec!["--version".to_string()],
        };

        let error = prober()
            .probe(&probe, Path::new("."))
            .expect_err("a probe for a tool that is not installed must fail");

        assert_eq!(error.code(), rskit_errors::ErrorCode::NotFound);
        let message = error.to_string();
        assert!(
            message.contains("toven-nonexistent-probe-tool") && message.contains("PATH"),
            "error must name the missing program and mention PATH: {message}"
        );
        assert!(
            error.cause().is_some(),
            "the underlying spawn failure must be preserved as the cause"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_present_tool_that_reports_no_version_is_tolerated() {
        let probe = ToolchainProbe {
            label: "check".to_string(),
            program: "false".to_string(),
            args: Vec::new(),
        };

        let version = prober()
            .probe(&probe, Path::new("."))
            .expect("a present tool that reports no version must not fail the probe");

        assert!(
            version.is_empty(),
            "no parseable version is expected, got {version:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_present_tool_that_exits_non_zero_yields_no_version_not_its_stdout() {
        let probe = ToolchainProbe {
            label: "check".to_string(),
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "printf boom; exit 3".to_string()],
        };

        let version = prober()
            .probe(&probe, Path::new("."))
            .expect("a present tool that exits non-zero must not fail the probe");

        assert!(
            version.is_empty(),
            "a non-zero exit must yield no version, got {version:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_probe_whose_stderr_overruns_the_cap_is_fatal() {
        let probe = ToolchainProbe {
            label: "check".to_string(),
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "head -c 200 /dev/zero 1>&2".to_string()],
        };

        let error =
            ProcessToolchainProber::with_max_output_bytes(Arc::new(ProcessToolRunner::new()), 64)
                .probe(&probe, Path::new("."))
                .expect_err("a stderr overrun must be a fatal probe error");

        assert_eq!(error.code(), rskit_errors::ErrorCode::Internal);
        assert!(
            error.to_string().contains("exceeded"),
            "the error must explain the output-cap overrun: {error}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_present_but_non_executable_probe_tool_propagates_the_classified_error() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let path = std::env::temp_dir().join(format!("toven-probe-noexec-{}", std::process::id()));
        let mut file = std::fs::File::create(&path).expect("create temp probe tool");
        file.write_all(b"#!/bin/sh\n")
            .expect("write temp probe tool");
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("drop the execute bit");

        let probe = ToolchainProbe {
            label: "check".to_string(),
            program: path.to_string_lossy().into_owned(),
            args: Vec::new(),
        };

        let error = prober()
            .probe(&probe, Path::new("."))
            .expect_err("a non-executable tool must fail the probe");

        std::fs::remove_file(&path).ok();

        assert_eq!(error.code(), rskit_errors::ErrorCode::Forbidden);
        assert!(
            !error.to_string().contains("PATH"),
            "a permission error must not be reported as a missing tool: {error}"
        );
    }
}
