//! `toven doctor`: the tool-audit verb.
//!
//! Answers "does this repository have the tools its tasks will run?" by
//! rendering the engine's [`ToolAudit`] — the classified, de-duplicated tool
//! set of the resolved task graph — through the same reporter sinks a run uses
//! (human on stderr, `--output jsonl` on stdout). The process exit is non-zero
//! when any required tool is missing, so `doctor` gates a script the way a
//! failing task would.
//!
//! Provisioning is an explicit opt-in: without `--ensure`/`--auto-install` the
//! verb is report-only (it never dials the network or installs anything). With
//! it, any missing tool is surfaced as a typed, actionable error, because the
//! per-task tools `doctor` audits (`cargo`, `ast-grep`, `mdbook`, …) are not
//! ecosystem drivers and Toven ships no provisioner for them — ecosystem-driver
//! provisioning lives under `toven driver`/`toven federation`. `doctor` never
//! fabricates an installer.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_engine::doctor::{ToolAudit, ToolProbeOutcome, audit_streaming};
use toven_engine::plan::ProcessToolchainProber;
use toven_model::{Event, ToolStatus};
use toven_ports::{Provider, Reporter};

use crate::host::{Project, Report};

/// `toven doctor [--ensure]`.
///
/// # Errors
/// Propagates configuration/probe failures from the audit, and — when `ensure`
/// is set and any tool is missing — the typed actionable error naming the tools
/// Toven cannot provision.
pub(crate) fn doctor(
    providers: &[&dyn Provider],
    project: &Project,
    report: Report,
    ensure: bool,
) -> AppResult<ExitCode> {
    let prober = ProcessToolchainProber::new();
    let mut reporter = report.reporter();
    // Stream each tool's verdict the moment its probe completes — the reporter
    // flushes per line — so `doctor` reports progressively (check → report →
    // next) like a run, instead of buffering every probe and dumping the audit
    // at the end.
    let audited = {
        let sink = reporter.as_mut();
        audit_streaming(
            &project.project_root,
            &project.document,
            providers,
            &prober,
            &mut |tool| emit_tool(sink, tool),
        )?
    };
    finish_audit(&audited, ensure, reporter.as_mut())
}

/// Emit one tool's verdict as an [`Event::ToolAudited`].
fn emit_tool(sink: &mut dyn Reporter, tool: &ToolProbeOutcome) -> AppResult<()> {
    sink.emit(&Event::ToolAudited {
        label: tool.label.clone(),
        program: tool.program.clone(),
        status: tool.status.clone(),
    })
}

/// Emit the terminal [`Event::DoctorFinished`] summary and resolve the process
/// exit: healthy → success; missing tools → failure (or, under `ensure`, the
/// typed unprovisionable error). Per-tool [`Event::ToolAudited`] events are
/// emitted as each probe completes (see [`emit_tool`]), so this only closes the
/// audit.
fn finish_audit(audit: &ToolAudit, ensure: bool, sink: &mut dyn Reporter) -> AppResult<ExitCode> {
    let missing = audit.missing_count();
    sink.emit(&Event::DoctorFinished {
        checked: audit.tools.len(),
        missing,
    })?;
    if missing == 0 {
        return Ok(ExitCode::Success);
    }
    if ensure {
        return Err(unprovisionable(audit));
    }
    Ok(ExitCode::Failure)
}

/// Project a fully-classified audit through `sink`: emit every per-tool verdict,
/// then close with [`finish_audit`]. The streaming [`doctor`] path emits the
/// per-tool events as probes complete; this batch projector keeps the same
/// event sequence available for unit tests without spawning real probes.
#[cfg(test)]
fn project_audit(audit: &ToolAudit, ensure: bool, sink: &mut dyn Reporter) -> AppResult<ExitCode> {
    for tool in &audit.tools {
        emit_tool(sink, tool)?;
    }
    finish_audit(audit, ensure, sink)
}

/// The typed, actionable error raised when `--ensure` cannot close the gap.
///
/// Names every missing tool so the operator can install them, and points at the
/// driver verbs for the only auto-provisionable surface. It never fabricates an
/// installer for a per-task tool.
fn unprovisionable(audit: &ToolAudit) -> AppError {
    let missing: Vec<&str> = audit
        .tools
        .iter()
        .filter(|tool| matches!(tool.status, ToolStatus::Missing))
        .map(|tool| tool.program.as_str())
        .collect();
    AppError::new(
        ErrorCode::NotFound,
        format!(
            "cannot provision {} missing tool(s) [{}]: Toven has no installer for per-task tools — \
             install them manually; ecosystem drivers are provisioned by `toven driver install`",
            missing.len(),
            missing.join(", "),
        ),
    )
}

#[cfg(test)]
mod tests {
    use rskit_errors::{AppResult, ErrorCode};
    use toven_engine::doctor::{ToolAudit, ToolProbeOutcome};
    use toven_model::{Event, ToolStatus};
    use toven_ports::Reporter;

    use super::project_audit;

    /// A minimal `Reporter` that records every emitted event, so the projection
    /// and exit policy are asserted without a real stdio sink (toven-cli's tests
    /// are self-contained and do not depend on toven-testkit).
    #[derive(Default)]
    struct CapturingReporter {
        events: Vec<Event>,
    }

    impl Reporter for CapturingReporter {
        fn emit(&mut self, event: &Event) -> AppResult<()> {
            self.events.push(event.clone());
            Ok(())
        }
    }

    fn present(program: &str) -> ToolProbeOutcome {
        ToolProbeOutcome::new(
            program,
            program,
            ToolStatus::Present {
                version: Some(format!("{program} 1.0")),
            },
        )
    }

    fn missing(program: &str) -> ToolProbeOutcome {
        ToolProbeOutcome::new(program, program, ToolStatus::Missing)
    }

    #[test]
    fn a_healthy_audit_emits_events_and_exits_success() {
        let audit = ToolAudit::new(vec![present("cargo")]);
        let mut reporter = CapturingReporter::default();

        let exit = project_audit(&audit, false, &mut reporter).expect("projects");

        assert_eq!(exit.as_i32(), 0);
        // One ToolAudited per tool plus the terminal DoctorFinished.
        assert_eq!(reporter.events.len(), 2);
    }

    #[test]
    fn a_missing_tool_exits_non_zero_in_report_only_mode() {
        let audit = ToolAudit::new(vec![present("cargo"), missing("mdbook")]);
        let mut reporter = CapturingReporter::default();

        let exit = project_audit(&audit, false, &mut reporter).expect("projects");

        assert_ne!(exit.as_i32(), 0);
        assert_eq!(reporter.events.len(), 3);
    }

    #[test]
    fn ensure_on_a_missing_tool_is_a_typed_actionable_error() {
        let audit = ToolAudit::new(vec![missing("mdbook")]);
        let mut reporter = CapturingReporter::default();

        let error = project_audit(&audit, true, &mut reporter)
            .expect_err("ensure must fail when a tool cannot be provisioned");

        assert_eq!(error.code(), ErrorCode::NotFound);
        assert!(
            error.to_string().contains("mdbook"),
            "the error must name the missing tool: {error}"
        );
        // The audit is still reported before the error is returned.
        assert_eq!(reporter.events.len(), 2);
    }
}
