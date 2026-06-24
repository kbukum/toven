//! Cache seam: the read-only [`CacheStore`] (PLAN) and write-only
//! [`CacheWriter`] (APPLY) halves.

mod store;
mod writer;

pub use store::CacheStore;
pub use writer::CacheWriter;
