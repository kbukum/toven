//! Shared human rendering: run headers, closing summary lines, outcome
//! tallies and the labels several actions print.

use rskit_cli::{ExitCode, Tone};
use toven_model::{Entrypoint, OutcomeSummary};
use toven_ports::PublicationPolicy;
use toven_release::{BumpReport, ReleasePlan, ReleaseStats};

use crate::flags::{ColorWhen, OutputKind};
use crate::report::stderr_theme;

/// Map a resolved release plan onto the shared item-based summary. A plan is a
/// read-only projection, so every entry is a `succeeded` item; a mutating
/// failure surfaces as an `Err`, never a `failed` summary count.
pub(super) const fn plan_outcome(plan: &ReleasePlan) -> OutcomeSummary {
    let processed = plan.entries.len();
    OutcomeSummary {
        processed,
        succeeded: processed,
        failed: 0,
        skipped: 0,
    }
}

/// Map a completed `release bump` report onto the shared item-based summary.
/// Every reported module reached a good terminal state (a mid-transaction
/// failure restores the tree and returns an `Err`), so all are `succeeded`.
pub(super) const fn bump_outcome(report: &BumpReport) -> OutcomeSummary {
    let processed = report.modules.len();
    OutcomeSummary {
        processed,
        succeeded: processed,
        failed: 0,
        skipped: 0,
    }
}

/// Map completed `release tag`/`publish` stats onto the shared item-based
/// summary. The transactional pipeline restores on any failure and returns an
/// `Err`, so a returned `ReleaseStats` counts only planned-and-completed
/// modules as `succeeded`.
pub(super) const fn run_outcome(stats: &ReleaseStats) -> OutcomeSummary {
    let processed = stats.planned_modules;
    OutcomeSummary {
        processed,
        succeeded: processed,
        failed: 0,
        skipped: 0,
    }
}

/// Derive the process exit from the shared item-based summary — the single
/// owner of the failure verdict (step-01 [`OutcomeSummary`]), so release, task
/// runs, and coverage all map their exit through one path rather than a
/// hardcoded [`ExitCode::Success`].
pub(super) const fn release_exit(summary: &OutcomeSummary) -> ExitCode {
    if summary.has_failures() {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

/// Print the release verb's header on stderr, human mode only.
///
/// A short title line rendered before the per-module decisions so they read as
/// a nested list; `--output jsonl` stays one record per module with no framing.
/// `action` is the canonical verb token (`plan`/`bump`/`tag`/`publish`).
pub(super) fn print_release_header(action: &str, output: OutputKind, color: ColorWhen) {
    if matches!(output, OutputKind::Human) {
        eprintln!(
            "{}",
            stderr_theme(color).heading(&format!("Release {action}"))
        );
    }
}

/// Print a terminal release summary line on stderr, human mode only.
///
/// The closing aggregate that matches the summary-derived exit; suppressed
/// under `--output jsonl` so the machine stream stays events-only.
pub(super) fn print_release_summary(line: &str, output: OutputKind, color: ColorWhen) {
    if matches!(output, OutputKind::Human) {
        eprintln!(
            "{}",
            stderr_theme(color).action("Finished", line, Tone::Success)
        );
    }
}

/// The truthful closing line for a `release plan` (or a mutating verb's
/// pre-confirmation preview), broken down by decision kind.
///
/// An empty plan states the up-to-date fact outright; a non-empty plan joins
/// only the non-zero groups (modules to release, dependency-floor-only moves,
/// already-released modules) so the reader scans exactly what will happen.
pub(super) fn plan_summary_line(plan: &ReleasePlan) -> String {
    if plan.is_empty() {
        return "release: nothing to release — all modules up to date".to_string();
    }
    let (mut to_release, mut floor_only, mut up_to_date) = (0_usize, 0_usize, 0_usize);
    for entry in &plan.entries {
        if entry.up_to_date {
            up_to_date += 1;
        } else if entry.planned_version.is_some() {
            to_release += 1;
        } else {
            floor_only += 1;
        }
    }
    let mut parts = Vec::new();
    if to_release > 0 {
        parts.push(format!("{to_release} to release"));
    }
    if floor_only > 0 {
        parts.push(format!("{floor_only} dependency floor"));
    }
    if up_to_date > 0 {
        parts.push(format!("{up_to_date} up to date"));
    }
    format!("release: {}", parts.join(", "))
}

/// The truthful closing line for a mutating version-cut verb, phrased by the
/// side effect that actually landed (`staged`/`tagged`/`published`).
pub(super) fn run_summary_line(action: &str, count: usize) -> String {
    let verb = match action {
        "bump" => "staged",
        "tag" => "tagged",
        "publish" => "published",
        _ => "released",
    };
    format!("release: {count} {verb}")
}

pub(super) fn publication_label(publication: &PublicationPolicy) -> String {
    publication.registry().map_or_else(
        || publication.as_str().to_string(),
        |registry| format!("{} ({registry})", publication.as_str()),
    )
}

/// Human label for a module's release flow in `release status`. For a
/// maintainer-owned module it surfaces whether the maintainer's release tag for
/// the declared version is already present — a fail-closed readiness signal
/// (`tag missing` means Toven cannot yet publish against it).
pub(super) fn status_flow_label(
    entrypoint: Entrypoint,
    maintainer_tag_present: Option<bool>,
) -> String {
    match maintainer_tag_present {
        Some(true) => format!("{} (tag ready)", entrypoint.as_str()),
        Some(false) => format!("{} (tag missing)", entrypoint.as_str()),
        None => entrypoint.as_str().to_string(),
    }
}

/// Render an optional pass/fail check for the human output.
pub(super) fn render_optional_check(value: Option<bool>) -> String {
    match value {
        Some(true) => "ok".to_string(),
        Some(false) => "FAIL".to_string(),
        None => "—".to_string(),
    }
}
