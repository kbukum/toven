//! The outcome of a single publish attempt.

use std::time::SystemTime;

/// The classified result of **one** publish attempt.
///
/// The port performs exactly one attempt and classifies the registry's
/// response; the engine owns the retry loop, topo ordering, idempotency
/// pre-skip, and budget. `publish` never waits internally.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PublishOutcome {
    /// The artifact was published successfully.
    Published,
    /// `name@version` was already published — idempotent skip (race/resume
    /// safety net).
    AlreadyPublished,
    /// The registry rate-limited the attempt.
    RateLimited {
        /// When to retry, parsed from the registry hint or the adapter's
        /// fallback cadence; `None` leaves the wait to the engine's default
        /// budget.
        retry_after: Option<SystemTime>,
    },
}
