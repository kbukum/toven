//! Library entrypoint for Toven.
//!
//! The public API exposes the CLI entrypoint and core planning contracts used
//! by upcoming discovery, scheduling, and rendering work.

pub mod cli;
pub mod config;
pub mod core;
pub mod preset;

/// Current package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
