//! Combined release facade: PLAN then APPLY in one call for the CLI `release`
//! verb.
//!
//! [`release_plan`](super::release_plan) and [`release_apply`](super::release_apply)
//! are exposed separately so each phase is testable in isolation, but a one-shot
//! `toven release` needs the discovered modules and resolved release targets that
//! the PLAN cut computes internally. This facade prepares the front matter once,
//! reuses it for both the plan and the apply, and returns the terminal
//! [`ReleaseStats`] — keeping the discovery/target wiring engine-owned so the CLI
//! stays a thin caller.

use rskit_errors::AppResult;
use toven_ports::{Provider, Reporter, VcsReader, VcsWriter};

use super::plan::{plan_with_context, release_targets};
use super::{ReleaseApplyOptions, ReleaseStats, release_apply};
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// Plan and apply a release in one call.
///
/// Prepares the shared PLAN front matter once, derives the release plan and
/// targets from it, then runs the release APPLY tail. `reader`/`writer` are the
/// two halves of the single git seam (the same adapter satisfies both).
///
/// # Errors
/// Propagates configuration/discovery/graph failures, release-plan failures, and
/// release-apply failures (guardrails, mutation, tagging, publishing).
pub fn release_run(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    reader: &dyn VcsReader,
    writer: &dyn VcsWriter,
    reporter: &mut dyn Reporter,
    options: &ReleaseApplyOptions,
) -> AppResult<ReleaseStats> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let plan = plan_with_context(&context, document, request, reader, &targets)?;
    release_apply(
        &plan,
        &context.federation.modules,
        &targets,
        reader,
        writer,
        options,
    )
}
