//! The release publish loop: registry idempotency pre-skip and bounded
//! rate-limit retry.
//!
//! The publish loop runs **after** the release commit, so a failure here never
//! rolls back history — it surfaces as a typed error. Idempotency is scoped to
//! the publish loop itself: re-attempting publish for the same module/version
//! pre-skips a version the registry already reports and treats an
//! [`AlreadyPublished`](PublishOutcome::AlreadyPublished) race as success. This
//! does not make the surrounding APPLY transaction (mutation/commit/tag/push)
//! resume-safe — rerunning the whole release is out of scope here.

use std::thread::sleep;
use std::time::{Duration, SystemTime};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{Artifact, PublishOutcome, ReleaseTarget};

use super::ReleaseStats;

/// Hard cap on a single rate-limit wait, regardless of the registry's hint, so a
/// pathological `Retry-After` cannot stall the publish loop indefinitely.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_mins(2);

/// One resolved unit of publish work, already ordered for deterministic publish.
pub(super) struct PublishItem<'a> {
    /// Module to publish.
    pub(super) module: &'a Module,
    /// Ecosystem release target for the module.
    pub(super) target: &'a dyn ReleaseTarget,
    /// Packaged artifact produced in the pre-commit phase.
    pub(super) artifact: &'a Artifact,
    /// Version being released.
    pub(super) version: &'a Version,
}

/// Publish each item in order, accounting outcomes into `stats`.
///
/// Idempotency: an item whose `version` the registry already reports is skipped
/// without a publish attempt. `AlreadyPublished` from a live attempt (a resume
/// race) is also treated as a resume-safe skip. `RateLimited` is retried within
/// `retry_budget`; an exhausted budget surfaces as a typed error.
pub(super) fn run(
    items: &[PublishItem<'_>],
    retry_budget: usize,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    for item in items {
        publish_one(item, retry_budget, stats)?;
    }
    Ok(())
}

fn publish_one(
    item: &PublishItem<'_>,
    retry_budget: usize,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    // Idempotency pre-skip: never re-publish a version the registry already has.
    // `published_versions` is best-effort (see `ReleaseTarget` docs): this loop
    // runs after the release commit, so a transient registry/search failure must
    // not abort APPLY. On lookup failure, fall through to a live publish attempt —
    // idempotency is preserved by the `AlreadyPublished` classification below.
    if item
        .target
        .published_versions(item.module)
        .is_ok_and(|versions| versions.contains(item.version))
    {
        stats.skipped_published_modules += 1;
        return Ok(());
    }

    let mut waits = 0_usize;
    loop {
        match item.target.publish(item.module, item.artifact)? {
            PublishOutcome::Published => {
                stats.published_modules += 1;
                return Ok(());
            }
            PublishOutcome::AlreadyPublished => {
                stats.skipped_published_modules += 1;
                return Ok(());
            }
            PublishOutcome::RateLimited { retry_after } => {
                if waits >= retry_budget {
                    return Err(rate_limit_exhausted(item, waits));
                }
                waits += 1;
                stats.rate_limited_waits += 1;
                wait_until(retry_after);
            }
        }
    }
}

/// Sleep until `retry_after`, bounded by [`MAX_RATE_LIMIT_WAIT`]. A `None` hint
/// retries immediately (the engine's budget alone bounds the loop), which also
/// keeps scripted tests free of real waits.
fn wait_until(retry_after: Option<SystemTime>) {
    let Some(at) = retry_after else {
        return;
    };
    if let Ok(remaining) = at.duration_since(SystemTime::now()) {
        sleep(remaining.min(MAX_RATE_LIMIT_WAIT));
    }
}

fn rate_limit_exhausted(item: &PublishItem<'_>, waits: usize) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!(
            "publishing '{}@{}' exhausted the rate-limit retry budget after {waits} wait(s)",
            item.module.id, item.version
        ),
    )
}
