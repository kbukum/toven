//! The per-unit cache verdict (closes the PLAN half).
//!
//! Maps the [`CacheMode`] and a content-key lookup to a static
//! [`CacheVerdict`]: a `Disabled` mode (or a uncacheable passthrough run) yields
//! `Disabled`, `Force` yields `Forced`, and `ReadWrite` resolves to `Hit`/`Miss`
//! by querying the [`CacheStore`].

use rskit_errors::AppResult;
use toven_model::CacheVerdict;
use toven_ports::CacheStore;

use super::super::request::CacheMode;

/// Decide the verdict for one unit given its mode, cacheability, and key.
///
/// `passthrough_present` with `cache_args == false` makes a `ReadWrite` unit
/// uncacheable (`Disabled`), since unhashed user args would otherwise alias
/// distinct runs to one record.
///
/// # Errors
/// Propagates a [`CacheStore`] lookup failure.
pub(in crate::plan) fn verdict(
    mode: CacheMode,
    cache_args: bool,
    passthrough_present: bool,
    key: &str,
    cache: &dyn CacheStore,
) -> AppResult<CacheVerdict> {
    match mode {
        CacheMode::Disabled => Ok(CacheVerdict::Disabled),
        CacheMode::Force => Ok(CacheVerdict::Forced),
        CacheMode::ReadWrite => {
            if passthrough_present && !cache_args {
                return Ok(CacheVerdict::Disabled);
            }
            if cache.contains(key)? {
                Ok(CacheVerdict::Hit)
            } else {
                Ok(CacheVerdict::Miss)
            }
        }
    }
}
