//! `toven-engine` — the engine orchestration layer.
//!
//! Layer 2 of the hexagonal architecture: the engine that drives the ports
//! ([`toven_ports`]) over the shared vocabulary ([`toven_model`]). It owns the
//! PLAN/APPLY spine; the later steps fill in discovery, scheduling, execution,
//! and release. This step lands the first behavior-bearing piece — the strict
//! configuration [`config::Document`] and its loader.
//!
//! Config is **engine-owned orchestration**: the engine owns the reserved-section
//! schemas and the one strict `rskit-config`-backed loader that parses the single
//! canonical `toven.toml` into a typed [`config::Document`]. The shared
//! `[ecosystems.<id>]` vocabulary ([`CommonEcosystemConfig`](toven_ports::CommonEcosystemConfig))
//! is owned by [`toven_ports`]; the engine keeps each dynamic-keyed subtree
//! verbatim for the adapter's own `configure` parse, which flattens that shared
//! surface.
//!
//! The engine also injects three IO ports defined in [`toven_ports`] —
//! [`ToolchainProber`](toven_ports::ToolchainProber),
//! [`SourceDigest`](toven_ports::SourceDigest), and
//! [`CacheStore`](toven_ports::CacheStore) — keeping the PLAN spine pure; their
//! concrete adapters ([`plan::ProcessToolchainProber`], [`plan::FsSourceDigest`],
//! [`plan::NullCache`]) live here in the engine.
//!
//! ## Modules
//! - [`config`] — the strict `Document`, reserved-section schemas, the
//!   structural-validation pass, the ecosystem-id three-way dispatch, and the
//!   `rskit-config::strict`-backed loader.
//! - [`vcs`] — the single git seam's implementation side: the rskit-git-backed
//!   [`vcs::RskitGitVcs`] adapter, the engine-owned [`vcs::BaselineStrategy`], and
//!   the per-repo [`vcs::VcsReaderSet`] dedup + fan-out.
//! - [`plan`] — the pure PLAN spine: the seven phases (Load → Configure →
//!   Discover → Graph → Affected → Toolchain → Schedule+Cache) that culminate in
//!   one immutable [`toven_model::Plan`]; it also hosts the concrete adapters for
//!   the injected toolchain/source/cache ports.
//! - [`output`] — the engine-owned per-unit raw child-output channel: buffers
//!   normal units into a labeled block (spilling extra blocks if a unit exceeds
//!   the per-unit buffer cap, to bound any single unit's buffer), live-tails
//!   persistent ones, and routes bytes through an
//!   injected `RawOutputSink` (the CLI renders; the engine does not print). The
//!   APPLY exec layer feeds it.
#![warn(missing_docs)]

pub mod config;
pub mod output;
pub mod plan;
pub mod vcs;
