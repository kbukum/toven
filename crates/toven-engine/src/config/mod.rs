//! Strict configuration: the `toven.toml` [`Document`], its reserved-section
//! schemas, structural validation, ecosystem-id dispatch, and the loader.
//!
//! Config is engine-owned orchestration (not a port): one canonical `toven.toml`
//! is parsed once into a strict, typed [`Document`]. Reserved sections
//! ([`ProjectConfig`], [`TovenConfig`], [`GroupConfig`], [`OverlayConfig`],
//! [`MemberConfig`]) carry engine schemas; each dynamic-keyed `[ecosystems.<id>]`
//! subtree is kept verbatim for the owning adapter's own strict parse.
//!
//! The loader is a thin wrapper over `rskit-config`'s strict loader (bounded reads,
//! `deny_unknown_fields`-honoring codec decode, verbatim raw subtrees,
//! identity-aware include-merge); the engine adds only Toven domain logic — the
//! reserved schemas, the `ecosystem:module` ref grammar, the structural-vs-semantic
//! split, and the two-registry ecosystem-id dispatch.

mod dispatch;
mod document;
mod group;
mod load;
mod member;
mod overlay;
mod project;
mod reference;
mod registry;
mod settings;
mod validate;

pub use dispatch::{Dispatch, dispatch};
pub use document::Document;
pub use group::{GroupConfig, Guardrails};
pub use load::{Loaded, load};
pub use member::MemberConfig;
pub use overlay::{OverlayConfig, OverlayRef};
pub use project::ProjectConfig;
pub use reference::ModuleRefSyntax;
pub use registry::CanonicalRegistry;
pub use settings::{CacheConfig, ReportFormat, TovenConfig, ViewMode};
