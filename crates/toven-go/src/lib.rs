//! `toven-go` — the Go ecosystem adapter.
//!
//! Implements the ecosystem ports against real `go` tooling. It depends only on
//! [`toven_ports`] + [`toven_model`] (never the engine) and is registered by id
//! `"go"`:
//!
//! - [`GoProvider`] parses `[ecosystems.go]` into a typed [`GoConfig`] and bakes
//!   a [`GoAdapter`]; it also drives the config-less init wizard for root
//!   `go.mod` projects.
//! - [`GoAdapter`] implements discovery (via `go mod edit -json` /
//!   `go work edit -json`), the toolchain probe, and run-strategy defaults. The
//!   runnable task table lives in `common().tasks`, authored by init or explicit
//!   config. Go modules release as VCS tags, so
//!   [`release_target`](toven_ports::ConfiguredAdapter::release_target) returns a
//!   [`GoVcsTarget`] that maps the root module to `vX.Y.Z` and submodules to
//!   `<path>/vX.Y.Z`.
//!
//! Discovery reads each managed `go.mod` offline (no module graph resolution,
//! no network). A root `go.work` is auto-detected both to enumerate the managed
//! modules under `modules = "auto"` and to group its members into one
//! workspace. All work returns typed data + typed errors; no user-facing
//! printing, no panics on runtime paths.

// The adapter's internal helpers live in private modules but are shared across
// sibling modules as `pub(crate)`. The `redundant_pub_crate` (nursery) lint would
// rather they be plain `pub`, but `unreachable_pub` then flags them as
// crate-internal — the two lints conflict for this shape. Allow the nursery lint
// crate-wide (the structure guard forbids per-`mod.rs` attributes) and keep the
// honest `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

mod adapter;
mod config;
mod detect;
mod discovery;
mod exec;
mod modules;
mod provider;
mod questionnaire;
mod release;
mod render;
mod tasks;
mod toolchain;

pub use adapter::GoAdapter;
pub use config::{GoConfig, Modules};
pub use provider::GoProvider;
pub use release::GoVcsTarget;
