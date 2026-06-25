//! `toven-command` — the command ecosystem adapter, Toven's argv-only escape
//! hatch.
//!
//! The deliberately-minimal sibling of the `toven-rust`/`toven-go` adapters: it
//! orchestrates arbitrary user-owned commands **without inferring anything**.
//! It depends only on [`toven_ports`] + [`toven_model`] (never the engine) and
//! is registered by id `"command"`:
//!
//! - [`CommandProvider`] parses `[ecosystems.command]` into a typed
//!   [`CommandConfig`] and bakes a [`CommandAdapter`]. There is no convention to
//!   auto-detect, so its `scaffold` always returns `None`.
//! - [`CommandAdapter`] normalizes the **declared** module/edge set (no tooling
//!   probe, no filesystem walk) and exposes **only** the user-declared
//!   `[tasks.*]` argv as tasks — no built-in build/test/lint defaults are
//!   invented. Its toolchain probe is the declared `[toolchain]` if present,
//!   else derived from the first declared task's program; release is out of
//!   scope, so [`release_target`](toven_ports::ConfiguredAdapter::release_target)
//!   is always `None`.
//!
//! Every command Toven runs through this adapter is user-owned argv
//! (argv-is-sacred). All work returns typed data + typed errors; no user-facing
//! printing, no panics on runtime paths.

// See the `toven-rust` lib note: `pub(crate)` helpers shared across sibling
// modules trip the `redundant_pub_crate` (nursery) lint, which conflicts with
// `unreachable_pub`. Allow the nursery lint crate-wide and keep honest
// `pub(crate)` visibility (the structure guard forbids per-`mod.rs` attributes).
#![allow(clippy::redundant_pub_crate)]

mod adapter;
mod config;
mod discovery;
mod provider;
mod tasks;

pub use adapter::CommandAdapter;
pub use config::{CommandConfig, DeclaredModule, DeclaredToolchain};
pub use provider::CommandProvider;
