//! Cache record writes after successful APPLY execution.

use rskit_errors::AppResult;
use toven_model::{CacheVerdict, ExecutionUnit};
use toven_ports::CacheWriter;

/// Record a cache key when a cacheable unit succeeds.
pub(super) fn record_success(unit: &ExecutionUnit, cache: &dyn CacheWriter) -> AppResult<()> {
    if matches!(unit.cache, CacheVerdict::Miss | CacheVerdict::Forced)
        && let Some(key) = unit.cache_key.as_deref()
    {
        cache.record(key)?;
    }
    Ok(())
}
