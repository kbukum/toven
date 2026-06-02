//! Rust discovery adapter.

pub mod cargo;
mod config;
mod discovery;
pub mod generate;
mod tasks;

pub use config::{RustProfileOptions, default_manifest};
pub use discovery::RustAdapter;
