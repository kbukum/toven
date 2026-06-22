//! `toven-engine` — the engine orchestration layer.
//!
//! Layer 3 of the hexagonal architecture: the engine that drives the ports
//! ([`toven_ports`]) over the shared vocabulary ([`toven_model`]). It owns the
//! PLAN/APPLY spine; the later steps fill in discovery, scheduling, execution,
//! and release. This step lands the first behavior-bearing piece — the strict
//! configuration [`config::Document`] and its loader.
//!
//! Config is **engine-owned orchestration**, not a port: there is one canonical
//! `toven.toml`, parsed once into a strict, typed [`config::Document`] whose
//! reserved sections the engine owns and whose dynamic-keyed `[ecosystems.<id>]`
//! subtrees are kept verbatim for each adapter's own `configure` parse.
//!
//! ## Modules
//! - [`config`] — the strict `Document`, reserved-section schemas, the
//!   structural-validation pass, the ecosystem-id three-way dispatch, and the
//!   `rskit-config::strict`-backed loader.
#![warn(missing_docs)]

pub mod config;
