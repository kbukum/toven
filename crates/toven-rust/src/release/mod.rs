//! `CargoRegistryTarget` — the cargo release-adapter sliver (crates.io by
//! default, or a named alternate registry).
//!
//! Owns the ecosystem-specific ~10% of release: reading a module's declared
//! version from its `Cargo.toml`, applying one atomic version mutation (own
//! version + intra-project dependency-floor rewrites) with format-preserving
//! `toml_edit`, and the registry-facing trio (`published_versions`, `package`,
//! `publish`) routed through `cargo` via `rskit-process` with bounded output
//! and a hard timeout. Each method returns typed data or a typed error — never
//! a success-shaped fallback.

mod manifest;
mod publish;
mod sbom;
mod target;

pub use target::CargoRegistryTarget;
