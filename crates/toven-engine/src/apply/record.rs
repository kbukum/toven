//! Cache record writes after successful APPLY execution.

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{CacheVerdict, ExecutionUnit};
use toven_ports::CacheWriter;

/// Record a cache key when a cacheable unit succeeds.
///
/// # Errors
/// A `Miss`/`Forced` verdict without a `cache_key` is an internal planner
/// invariant violation: the unit was decided cacheable but carries no key to
/// write, so it returns an error rather than reporting silent success.
pub(super) fn record_success(unit: &ExecutionUnit, cache: &dyn CacheWriter) -> AppResult<()> {
    if matches!(unit.cache, CacheVerdict::Miss | CacheVerdict::Forced) {
        let key = unit.cache_key.as_deref().ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "unit '{}' has a cacheable verdict but no cache key to record",
                    unit.id
                ),
            )
        })?;
        cache.record(key)?;
    }
    Ok(())
}
