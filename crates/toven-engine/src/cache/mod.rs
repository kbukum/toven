//! The engine's concrete cache backend.
//!
//! The cache *ports* ([`CacheStore`](toven_ports::CacheStore) read,
//! [`CacheWriter`](toven_ports::CacheWriter) write) live in `toven-ports`; their
//! concrete filesystem adapter lives here, in the consuming layer. PLAN injects
//! the read half and APPLY injects the write half — one [`FsContentCache`] can
//! serve both.

mod fs;

pub use fs::FsContentCache;
