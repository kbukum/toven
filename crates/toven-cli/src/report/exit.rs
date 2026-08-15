//! [`exit_code`] — map a terminal [`RunStats`] summary to a process exit code.

use rskit_cli::ExitCode;
use toven_model::RunStats;

/// Derive the process exit code from a run's `RunFinished` summary.
///
/// Non-zero ([`ExitCode::Failure`]) if any unit ended `Failed`, `Blocked`,
/// `FailedReadiness`, or `TimedOut`; otherwise zero ([`ExitCode::Success`]). An
/// all-cached run and a persistent-goal run that held until signal and shut
/// down cleanly both map to success; a held unit that crashed surfaces as a
/// failure counter and therefore non-zero.
#[must_use]
pub const fn exit_code(summary: &RunStats) -> ExitCode {
    if summary.has_failures() {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

/// Derive the terminal process exit code, honoring a graceful-stop signal.
///
/// A cooperatively cancelled run (the shared shutdown token fired on
/// SIGINT/SIGTERM/SIGHUP) exits [`ExitCode::Cancelled`] (`130`) regardless of the
/// summary counters: cancellation records `cancelled_units`, which
/// [`RunStats::has_failures`] deliberately excludes, so without this an
/// interrupted-but-otherwise-healthy run would misreport success (`0`).
/// Cancellation takes precedence so the documented "graceful stop → 130"
/// contract holds; an uninterrupted run falls through to [`exit_code`].
#[must_use]
pub const fn terminal_exit_code(summary: &RunStats, cancelled: bool) -> ExitCode {
    if cancelled {
        ExitCode::Cancelled
    } else {
        exit_code(summary)
    }
}

#[cfg(test)]
mod tests {
    use rskit_cli::ExitCode;
    use toven_model::RunStats;

    use super::exit_code;

    #[test]
    fn all_cached_run_exits_zero() {
        let mut stats = RunStats::new(3);
        stats.cached_units = 3;
        stats.cache_hits = 3;
        assert_eq!(exit_code(&stats), ExitCode::Success);
    }

    #[test]
    fn clean_persistent_shutdown_exits_zero() {
        // Persistent units that became ready and tore down cleanly leave no failure
        // counters set.
        let stats = RunStats::new(1);
        assert_eq!(exit_code(&stats), ExitCode::Success);
    }

    #[test]
    fn failed_unit_exits_non_zero() {
        let mut stats = RunStats::new(2);
        stats.failed_units = 1;
        assert_eq!(exit_code(&stats), ExitCode::Failure);
    }

    #[test]
    fn blocked_unit_exits_non_zero() {
        let mut stats = RunStats::new(2);
        stats.blocked_units = 1;
        assert_eq!(exit_code(&stats), ExitCode::Failure);
    }

    #[test]
    fn failed_readiness_exits_non_zero() {
        let mut stats = RunStats::new(1);
        stats.failed_readiness_units = 1;
        assert_eq!(exit_code(&stats), ExitCode::Failure);
    }

    #[test]
    fn timed_out_unit_exits_non_zero() {
        let mut stats = RunStats::new(1);
        stats.timed_out_units = 1;
        assert_eq!(exit_code(&stats), ExitCode::Failure);
    }

    #[test]
    fn an_interrupted_healthy_run_exits_cancelled() {
        // Cancellation records `cancelled_units`, which `has_failures` excludes,
        // so an otherwise-healthy interrupted run must still map to 130 — not the
        // summary-derived success — when the shutdown token fired.
        let mut stats = RunStats::new(2);
        stats.cancelled_units = 1;
        assert_eq!(super::terminal_exit_code(&stats, true), ExitCode::Cancelled);
    }

    #[test]
    fn an_uninterrupted_run_falls_through_to_the_summary_code() {
        let mut stats = RunStats::new(1);
        stats.failed_units = 1;
        assert_eq!(super::terminal_exit_code(&stats, false), ExitCode::Failure);

        let clean = RunStats::new(1);
        assert_eq!(super::terminal_exit_code(&clean, false), ExitCode::Success);
    }
}
