//! The engine's concrete cache backend.
//!
//! The cache *ports* ([`CacheStore`](toven_ports::CacheStore) read,
//! [`CacheWriter`](toven_ports::CacheWriter) write) live in `toven-ports`;
//! their concrete adapters live here, in the consuming layer. PLAN injects the
//! read half and APPLY injects the write half — one [`FsContentCache`] can serve
//! both, while [`NullCache`] supplies the no-backend PLAN default.

mod fs;
mod null;
mod root;

pub use fs::FsContentCache;
pub use null::NullCache;
pub use root::{CACHE_DIR_ENV, CACHE_FORMAT_VERSION, resolve_root};
