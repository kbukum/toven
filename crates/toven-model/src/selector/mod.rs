//! Selector grammar: the lenient, pre-resolution shape of a module reference.
//!
//! The user-facing selection boundary accepts the shortest unambiguous module
//! reference (`api`, `rust:core`, `backend/api`, `rust:*`, `rskit-*`); this
//! module parses such a token into a [`ModuleSelector`] pattern. Resolving a
//! pattern into concrete module keys — including the ambiguity and empty-match
//! errors — belongs to the engine, the layer that owns a graph. Canonical
//! `ecosystem:name` identity ([`ModuleRef`](crate::ModuleRef)) is unchanged and
//! stays the output form everywhere.

mod pattern;
mod reference;

pub use pattern::NamePattern;
pub use reference::ModuleSelector;
