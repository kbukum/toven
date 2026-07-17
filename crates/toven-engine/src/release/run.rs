//! Combined release facade: PLAN then APPLY in one call for the CLI `release`
//! verb.
//!
//! [`release_plan`](super::release_plan) and the per-member APPLY are exposed
//! separately so each phase is testable in isolation, but a one-shot
//! `toven release` needs the discovered modules and resolved release targets that
//! the PLAN cut computes internally. This facade prepares the front matter once,
//! reuses it for both the plan and the apply, and returns the terminal
//! [`ReleaseStats`] — keeping the discovery/target wiring engine-owned so the CLI
//! stays a thin caller.

use rskit_errors::AppResult;
use toven_ports::{Provider, Reporter};

use super::plan::{plan_with_context, release_targets};
use super::{BumpOverrides, ReleaseApplyOptions, ReleaseStats};
use crate::config::Document;
use crate::federation::baseline::MemberVcsReaders;
use crate::federation::release::{MemberReleaseRepos, release_apply_by_member};
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// Plan and apply a release in one call.
///
/// Prepares the shared PLAN front matter once, derives the release plan and
/// targets from it, then runs the per-member release APPLY tail. `readers` are
/// the per-member change seams and `repos` the per-member commit/tag/push ports;
/// a single-repo project is the N=1 degenerate member. `overrides` carry the
/// per-run bump argv (level flags, set-version, prerelease channel, base,
/// offline).
///
/// # Errors
/// Propagates configuration/discovery/graph failures, release-plan failures, and
/// release-apply failures (guardrails, mutation, tagging, publishing).
#[allow(clippy::too_many_arguments)]
pub fn release_run(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    repos: &MemberReleaseRepos<'_>,
    overrides: &BumpOverrides,
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
    let plan = plan_with_context(&context, document, request, readers, overrides, &targets)?;
    release_apply_by_member(&plan, &context.federation.modules, &targets, repos, options)
}
