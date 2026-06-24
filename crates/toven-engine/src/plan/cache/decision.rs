//! The per-unit cache verdict (closes the PLAN half).
//!
//! Maps the [`CacheMode`] and a lazily computed content key to a static
//! [`CacheVerdict`]: a `Disabled` mode (or a uncacheable passthrough run) yields
//! `Disabled` without deriving a key, `Force` yields `Forced`, and `ReadWrite`
//! resolves to `Hit`/`Miss` by querying the [`CacheStore`].

use rskit_errors::AppResult;
use toven_model::CacheVerdict;
use toven_ports::CacheStore;

use super::super::request::CacheMode;

/// Decide the verdict for one unit given its mode and cacheability.
///
/// `passthrough_present` with `cache_args == false` makes a `ReadWrite` unit
/// uncacheable (`Disabled`), since unhashed user args would otherwise alias
/// distinct runs to one record.
///
/// The content key is computed lazily via `compute_key` and only when the
/// verdict actually needs it (a `Force` record or a `ReadWrite` store lookup):
/// for every `Disabled` outcome the key is never derived, avoiding wasted
/// digest work and the filesystem/digest errors it could surface for a unit the
/// cache will never consult. The returned `Option<String>` carries the computed
/// key (when one was derived) so callers can store it without recomputing.
///
/// # Errors
/// Propagates a `compute_key` digest failure or a [`CacheStore`] lookup failure.
pub(in crate::plan) fn verdict<F>(
    mode: CacheMode,
    cache_args: bool,
    passthrough_present: bool,
    cache: &dyn CacheStore,
    compute_key: F,
) -> AppResult<(CacheVerdict, Option<String>)>
where
    F: FnOnce() -> AppResult<String>,
{
    match mode {
        CacheMode::Disabled => Ok((CacheVerdict::Disabled, None)),
        CacheMode::Force => Ok((CacheVerdict::Forced, Some(compute_key()?))),
        CacheMode::ReadWrite => {
            if passthrough_present && !cache_args {
                return Ok((CacheVerdict::Disabled, None));
            }
            let key = compute_key()?;
            let verdict = if cache.contains(&key)? {
                CacheVerdict::Hit
            } else {
                CacheVerdict::Miss
            };
            Ok((verdict, Some(key)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use toven_testkit::FakeCacheStore;

    use super::{CacheMode, CacheVerdict, verdict};

    #[test]
    fn disabled_mode_yields_disabled_without_deriving_a_key() {
        let cache = FakeCacheStore::new();
        let computed = Cell::new(false);
        let (decided, key) = verdict(CacheMode::Disabled, true, true, &cache, || {
            computed.set(true);
            Ok("k".to_string())
        })
        .unwrap();
        assert_eq!(decided, CacheVerdict::Disabled);
        assert!(key.is_none());
        assert!(
            !computed.get(),
            "no key for a deterministically Disabled verdict"
        );
    }

    #[test]
    fn readwrite_passthrough_without_cache_args_disables_without_key() {
        let cache = FakeCacheStore::new();
        let computed = Cell::new(false);
        let (decided, key) = verdict(CacheMode::ReadWrite, false, true, &cache, || {
            computed.set(true);
            Ok("k".to_string())
        })
        .unwrap();
        assert_eq!(decided, CacheVerdict::Disabled);
        assert!(key.is_none());
        assert!(
            !computed.get(),
            "uncacheable passthrough run derives no key"
        );
    }

    #[test]
    fn force_mode_derives_key_and_marks_forced() {
        let cache = FakeCacheStore::new();
        let (decided, key) = verdict(CacheMode::Force, false, false, &cache, || {
            Ok("k".to_string())
        })
        .unwrap();
        assert_eq!(decided, CacheVerdict::Forced);
        assert_eq!(key.as_deref(), Some("k"));
    }

    #[test]
    fn readwrite_resolves_hit_and_miss_against_the_store() {
        let present = FakeCacheStore::new().with_key("k");
        let (decided, key) = verdict(CacheMode::ReadWrite, false, false, &present, || {
            Ok("k".to_string())
        })
        .unwrap();
        assert_eq!(decided, CacheVerdict::Hit);
        assert_eq!(key.as_deref(), Some("k"));

        let empty = FakeCacheStore::new();
        let (decided, key) = verdict(CacheMode::ReadWrite, false, false, &empty, || {
            Ok("k".to_string())
        })
        .unwrap();
        assert_eq!(decided, CacheVerdict::Miss);
        assert_eq!(key.as_deref(), Some("k"));
    }
}
