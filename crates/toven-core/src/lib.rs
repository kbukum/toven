//! `toven-core` — the shared PLAN foundation below the engine.
//!
//! Layer 2a of the hexagonal architecture: the lowest engine crate, sitting
//! directly on the ports ([`toven_ports`]) and the shared vocabulary
//! ([`toven_model`]). It owns the config `Document`, the git seam, the pure
//! PLAN spine, and umbrella federation — the concerns that both the slimmed
//! [`toven-engine`](../toven_engine/index.html) and
//! [`toven-release`](../toven_release/index.html) crates depend
//! downward on. Layering stays downward-only: this crate imports **no**
//! `toven-engine`, `toven-release`, or `toven-cli` types.
//!
//! Config is **engine-owned orchestration**: this crate owns the
//! reserved-section schemas and the one strict `rskit-config`-backed loader
//! that parses the single canonical `toven.toml` into a typed
//! [`config::Document`]. The shared `[ecosystems.<id>]` vocabulary
//! ([`CommonEcosystemConfig`](toven_ports::CommonEcosystemConfig)) is owned by
//! [`toven_ports`]; the loader keeps each dynamic-keyed subtree verbatim for
//! the adapter's own `configure` parse, which flattens that shared surface.
//!
//! The PLAN spine injects two IO ports defined in [`toven_ports`] —
//! [`ToolchainProber`](toven_ports::ToolchainProber) and
//! [`SourceDigest`](toven_ports::SourceDigest) — keeping the phases pure; their
//! concrete adapters ([`plan::ProcessToolchainProber`],
//! [`plan::FsSourceDigest`], [`plan::NullCache`]) live here.
//!
//! ## Modules
//! - [`config`] — the strict `Document`, reserved-section schemas, the
//!   structural-validation pass, the ecosystem-id three-way dispatch, and the
//!   `rskit-config::strict`-backed loader.
//! - [`vcs`] — the engine-owned baseline *policy*
//!   ([`vcs::BaselineStrategy`]) over the git seam; the git mechanism itself
//!   (the rskit-git-backed adapter, change foundation, and per-repo reader-set
//!   fan-out) lives in the focused [`toven-vcs`](../toven_vcs/index.html) crate.
//! - [`plan`] — the pure PLAN spine: the seven phases (Load → Configure →
//!   Discover → Graph → Affected → Toolchain → Schedule+Cache) that culminate
//!   in one immutable [`toven_model::Plan`]; it also hosts the concrete
//!   adapters for the injected toolchain/source/cache ports.
//! - [`federation`] — umbrella federation: in-proc adapters plus the
//!   [`RemoteAdapter`](federation::RemoteAdapter) proxy that drives a
//!   separately installed `toven-<eco> __serve` over a thin framed stdio
//!   transport, the four-way driver dispatch, the `__serve` port-server loop,
//!   and the explicit driver provisioning surface.
#![warn(missing_docs)]

pub mod config;
pub mod federation;
pub mod plan;
pub mod vcs;
