//! The standalone `release bump` verb: the version + changelog mutation phase
//! run on its own.
//!
//! `bump` performs **only** the `bump` phase — it rewrites each module's
//! manifest version and dependency floors and, where configured, rolls the
//! changelog, then either creates the release commit (the default) or leaves the
//! mutation staged for a maintainer's pull request (`--no-commit`). It never
//! tags, pushes, publishes, or cuts a hosted Release: in the real Toven/rskit
//! flow the version/CHANGELOG change *is* the release decision, and tag/publish
//! come after it merges.
//!
//! Like [`release_run`](super::release_run) this facade prepares the shared PLAN
//! front matter once, derives the plan and targets from it, then runs the
//! per-member mutation tail — keeping the discovery/target wiring engine-owned
//! so the CLI stays a thin caller.

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_util::time::{Clock, datetime_from_epoch_secs};
use rskit_version::semver::Version;
use toven_model::ModuleKey;
use toven_ports::{Provider, Reporter};

use super::BumpOverrides;
use super::plan::{plan_with_context, release_targets};
use toven_engine_core::config::Document;
use toven_engine_core::federation::baseline::MemberVcsReaders;
use toven_engine_core::federation::member_repo::MemberReleaseRepos;
use crate::release::federated::release_bump_by_member;
use toven_engine_core::federation::resolve::PathDriverLocator;
use toven_engine_core::plan::{PlanRequest, prepare_front};

/// Runtime options for the standalone `release bump` phase.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub struct BumpOptions {
    /// Stage the version/changelog mutation for a pull request instead of
    /// creating the release commit.
    pub no_commit: bool,
    /// Preview the mutation without writing manifests, the changelog, or git
    /// state.
    pub dry_run: bool,
}

/// One module's `bump` outcome: its version transition and the manifest paths
/// the mutation rewrote.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BumpModuleOutcome {
    /// The bumped module.
    pub module: ModuleKey,
    /// The module's version before the bump.
    pub old_version: Version,
    /// The module's version after the bump.
    pub new_version: Version,
    /// Repo-relative manifest paths the mutation rewrote (empty under
    /// `--dry-run`, which previews without writing, and for a mutation-free
    /// tag-only ecosystem).
    pub manifests: Vec<String>,
}

/// The typed result of a `release bump` run.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct BumpReport {
    /// Whether the run created the release commit. `false` when `--no-commit`
    /// staged the change for a pull request or `--dry-run` previewed it.
    pub committed: bool,
    /// Whether the run was a mutation-free preview.
    pub dry_run: bool,
    /// Per-module version transitions in plan order.
    pub modules: Vec<BumpModuleOutcome>,
    /// Repo-relative changelog paths rolled into a versioned section (or, under
    /// `--dry-run`, the paths that would roll).
    pub changelogs: Vec<String>,
}

impl BumpReport {
    /// A report for a run that committed nothing yet — the initial state the
    /// per-member tail builds on, and the terminal report for an up-to-date
    /// project with nothing to bump. `committed` starts `false` and is set only
    /// once a member actually creates the release commit, so a no-op run, a
    /// `--no-commit` staging run, and a `--dry-run` preview all report
    /// `committed = false` truthfully.
    #[must_use]
    pub(crate) const fn empty(options: BumpOptions) -> Self {
        Self {
            committed: false,
            dry_run: options.dry_run,
            modules: Vec::new(),
            changelogs: Vec::new(),
        }
    }
}

/// Run the standalone `bump` phase: plan the release, then mutate manifests and
/// roll the changelog per member, creating the release commit or staging it for
/// a pull request.
///
/// `readers` are the per-member change seams and `repos` the per-member
/// commit/stage ports; a single-repo project is the N=1 degenerate member.
/// `overrides` carry the per-run bump argv (level flags, set-version, prerelease
/// channel, base, offline). `clock` supplies the date stamped into a rolled
/// changelog's versioned heading.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, release-plan failures, and
/// mutation failures (clean-tree/branch guardrails, manifest mutation, changelog
/// roll, staging or commit).
#[allow(clippy::too_many_arguments)]
pub fn release_bump(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    repos: &MemberReleaseRepos<'_>,
    overrides: &BumpOverrides,
    reporter: &mut dyn Reporter,
    clock: &dyn Clock,
    options: &BumpOptions,
) -> AppResult<BumpReport> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let plan = plan_with_context(
        &context,
        request,
        readers,
        overrides,
        &targets,
        super::bump::CutIntent::Mutate,
    )?;
    let date = today(clock)?;
    release_bump_by_member(
        &plan,
        &context.federation.modules,
        &targets,
        repos,
        &date,
        *options,
    )
}

/// Format the clock's current UTC civil date as `YYYY-MM-DD` for a rolled
/// changelog's versioned heading.
///
/// # Errors
/// Fails when the clock's epoch second count does not fit in an `i64` rather
/// than silently stamping a fallback date.
fn today(clock: &dyn Clock) -> AppResult<String> {
    let seconds = i64::try_from(clock.epoch_seconds()).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            "clock epoch seconds do not fit in a signed 64-bit civil-date conversion",
        )
        .with_cause(error)
    })?;
    let date = datetime_from_epoch_secs(seconds).date;
    Ok(format!(
        "{:04}-{:02}-{:02}",
        date.year, date.month, date.day
    ))
}

#[cfg(test)]
mod tests {
    use super::{BumpOptions, BumpReport};

    #[test]
    fn empty_report_never_claims_a_commit() {
        // A run with nothing to bump commits nothing, so the terminal report
        // must read `committed = false` for every disposition — including the
        // default (commit) options, which only earns `committed` once a member
        // actually creates the release commit.
        for options in [
            BumpOptions::default(),
            BumpOptions {
                no_commit: true,
                dry_run: false,
            },
            BumpOptions {
                no_commit: false,
                dry_run: true,
            },
        ] {
            let report = BumpReport::empty(options);
            assert!(
                !report.committed,
                "empty report claims a commit: {report:?}"
            );
            assert_eq!(report.dry_run, options.dry_run);
            assert!(report.modules.is_empty());
            assert!(report.changelogs.is_empty());
        }
    }
}
