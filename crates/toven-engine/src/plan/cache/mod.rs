//! The Cache-decision surface: the content key, the lookup port, and the verdict.

mod decision;
mod key;
mod store;

pub(in crate::plan) use decision::verdict;
pub(in crate::plan) use key::{
    KeyInputs, forward_adjacency, needed_modules, source_hashes, unit_key,
};
pub use store::NullCache;
