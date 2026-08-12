//! `toven-model` — the shared Toven vocabulary.
//!
//! Layer 0 of the hexagonal architecture: the types everyone speaks plus the
//! pure graph algorithms that ports and the engine consume. It is the
//! dependency root, depending only on `rskit-errors` (the error contract) and
//! `rskit-validation` (identifier validation) — never on ports, adapters, or
//! the engine.
//!
//! All fallible APIs return [`rskit_errors::AppResult`]; there is no bespoke
//! error type. Every cross-boundary type is `serde`-serializable so it survives
//! the out-of-process driver transport.
//!
//! ## Modules
//! - [`identity`] — [`EcosystemId`], [`ModuleRef`], [`ModuleKey`],
//!   [`WorkspaceId`], [`MemberId`], [`RepoPath`], [`AbsPath`].
//! - [`module`] / [`edge`] / [`workspace`] — [`Module`], [`Edge`]/[`DepKind`],
//!   [`Workspace`]/[`ToolchainTag`].
//! - [`graph`] — [`Graph`] build/validate + wave-leveling + reverse closure.
//! - [`plan`] — [`Plan`], [`ExecutionUnit`], [`CacheVerdict`],
//!   [`ExecutionReadiness`], [`TaskOrigin`].
//! - [`selector`] — [`ModuleSelector`], [`NamePattern`] (the lenient selection
//!   grammar).
//! - [`event`] — [`Event`], [`UnitStatus`], [`RunStats`].
//! - [`ecosystems`] — the canonical ecosystem registry.
//! - [`release`] — [`ReleasePhase`] and [`Entrypoint`], the release-flow
//!   stage and entrypoint vocabulary.
//! - [`mod@unit`] — [`Unit`], [`Backing`], [`Composite`]: the one action shape
//!   every capability takes and how it is satisfied.

pub mod ecosystems;
pub mod edge;
pub mod event;
pub mod graph;
pub mod identity;
pub mod module;
pub mod plan;
pub mod release;
pub mod selector;
pub mod tool;
pub mod unit;
pub mod workspace;

pub use ecosystems::{CanonicalEcosystem, canonical_ecosystems};
pub use edge::{DepKind, Edge};
pub use event::{Event, OutputStream, Phase, RunStats, UnitOutput, UnitStatus};
pub use graph::Graph;
pub use identity::{AbsPath, EcosystemId, MemberId, ModuleKey, ModuleRef, RepoPath, WorkspaceId};
pub use module::Module;
pub use plan::{CacheVerdict, ExecutionReadiness, ExecutionUnit, Plan, TaskOrigin};
pub use release::{Entrypoint, ReleasePhase};
pub use selector::{ModuleSelector, NamePattern};
pub use tool::ToolStatus;
pub use unit::{Backing, Composite, Unit};
pub use workspace::{ToolchainTag, Workspace};
