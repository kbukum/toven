//! Library entrypoint for Toven.
//!
//! The public API is intentionally small while the project foundation lands.
//! Planning, discovery, and execution contracts will be added through focused
//! follow-up pull requests.

pub mod cli;
pub mod core;

/// Current package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
