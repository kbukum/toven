//! `toven-rust` — the Rust/cargo ecosystem adapter.
//!
//! Implements the ecosystem ports against real cargo tooling. It depends only
//! on [`toven_ports`] + [`toven_model`] (never the engine) and is registered by
//! id `"rust"`:
//!
//! - [`RustProvider`] parses `[ecosystems.rust]` into a typed [`RustConfig`]
//!   and bakes a [`RustAdapter`]; it also self-detects a Cargo project and
//!   drives the `toven init` onboarding wizard (detect → questionnaire →
//!   render).
//! - [`RustAdapter`] implements discovery (via `cargo metadata`), the toolchain
//!   probe, run-strategy defaults, and the crates.io release target. The
//!   runnable task table is read from the authoritative config, not the
//!   adapter.
//!
//! All work returns typed data + typed errors; no user-facing printing, no
//! panics on runtime paths.

// The adapter's internal helpers live in private modules but are shared across sibling modules as
// `pub(crate)`. The `redundant_pub_crate` (nursery) lint would rather they be plain `pub`, but
// `unreachable_pub` then flags them as crate-internal — the two lints conflict for this shape.
// Allow the nursery lint crate-wide (the structure guard forbids per-`mod.rs` attributes) and keep
// the honest `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

mod adapter;
mod config;
mod detect;
mod discovery;
mod manifests;
mod provider;
mod questionnaire;
mod release;
mod render;
mod tasks;
mod toolchain;

pub use adapter::RustAdapter;
pub use config::{Manifests, RustConfig};
pub use provider::RustProvider;
pub use release::CratesIoTarget;
