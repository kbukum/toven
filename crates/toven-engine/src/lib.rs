//! `toven-engine` — the engine orchestration layer.
//!
//! Layer 2b of the hexagonal architecture: the engine that drives the ports
//! ([`toven_ports`]) over the shared vocabulary ([`toven_model`]) on top of the
//! shared PLAN foundation ([`toven_core`]). It owns the APPLY execution
//! tail plus the standalone engine concerns and concrete adapters — apply,
//! cache, coverage, output, source digest, toolchain probing, watch, init, and
//! doctor — while config, the VCS seam, the PLAN spine, and umbrella federation
//! live in [`toven_core`] and the release tail lives in
//! [`toven_release`](../toven_release/index.html).
//!
//! The engine injects the write side of the cache port defined in
//! [`toven_ports`] — [`CacheWriter`](toven_ports::CacheWriter) — over the pure
//! PLAN spine that [`toven_core`] owns; the concrete filesystem backend
//! ([`cache::FsContentCache`]) lives here.
//!
//! ## Modules
//! - [`output`] — the engine-owned per-unit raw child-output channel: buffers
//!   normal units into a labeled block (spilling extra blocks if a unit exceeds
//!   the per-unit buffer cap, to bound any single unit's buffer), live-tails
//!   persistent ones, and routes bytes through an injected `RawOutputSink` (the
//!   CLI renders; the engine does not print). The APPLY exec layer feeds it.
//! - [`cache`] — the concrete filesystem cache backend
//!   ([`cache::FsContentCache`]) implementing both injected cache ports: the
//!   read-only [`CacheStore`](toven_ports::CacheStore) for PLAN and the
//!   write-only [`CacheWriter`](toven_ports::CacheWriter) for APPLY, plus the
//!   no-backend [`cache::NullCache`] PLAN default.
//! - [`source`] — the bounded filesystem-backed
//!   [`SourceDigest`](toven_ports::SourceDigest) adapter.
//! - [`toolchain`] — the bounded process-backed
//!   [`ToolchainProber`](toven_ports::ToolchainProber) adapter.
//! - [`apply`] — the APPLY execution layer: schedules and runs the immutable
//!   [`toven_model::Plan`] over the injected ports, feeding the output channel.
//! - [`coverage`] — the coverage PLAN/APPLY tail over the shared spine.
//! - [`watch`] — watch mode: the [`watch::WatchSession`] PLAN→APPLY rerun loop
//!   over the injected [`WatchSource`](toven_ports::WatchSource) port plus the
//!   concrete rskit-fs [`watch::RskitFsWatch`] adapter.
//! - [`init`] — project scaffolding: the guided `toven.toml` init flow.
//! - [`doctor`] — the diagnostics pass that reuses the Configure phase to bake
//!   every declared ecosystem and surface configuration problems.
#![warn(missing_docs)]

pub mod apply;
pub mod cache;
pub mod coverage;
pub mod doctor;
pub mod init;
pub mod output;
pub mod source;
pub mod toolchain;
pub mod watch;
