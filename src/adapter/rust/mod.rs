//! Rust discovery adapter.

pub mod cargo;
mod config;
mod discovery;

pub use config::{RustProfileOptions, default_manifest};
pub use discovery::RustAdapter;
