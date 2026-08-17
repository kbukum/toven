//! The pure PLAN spine: the seven phases (Load → Configure → Discover → Graph →
//! Affected → Toolchain → Schedule+Cache) culminating in one immutable
//! [`Plan`](toven_model::Plan).
//!
//! Load is the [`config`](crate::config) loader; the remaining phases live
//! one-per-file under this module and are driven by [`pipeline::plan`]. The
//! cross-language differentiator is built here: ONE federated dependency graph,
//! assembled centrally before affected/schedule.
//!
//! ## Surface
//! - `pipeline` — drives the phases and emits PHASE/PLAN events →
//!   [`Plan`](toven_model::Plan).
//! - `configure` — bakes each ecosystem subtree into a `ConfiguredAdapter`.
//! - `discover` — full federation union (workspaces/modules/edges + overlays).
//! - `graph` — `Graph::build` plus the deferred SEMANTIC config validation.
//! - `affected` — resolve a request's selection to the active module set
//!   (explicit selectors / changed-path composition + closure).
//! - `ownership` — the shared path→owning-module resolver (longest-prefix roots
//!   + blast radius) consumed by both affected-selection and release.
//! - `toolchain` — per-active-workspace `{tool, version}` resolution.
//! - `schedule` — `RunStrategy` relaxation → federated waves → per-module
//!   units.
//! - `cache` — the content key, lookup port, and per-unit verdict.
//! - `request` / `host` — PLAN inputs and injected ports.

pub mod affected;
mod cache;
pub(crate) mod catalog;
pub mod configure;
pub(crate) mod discover;
pub(crate) mod front;
mod graph;
mod host;
mod overrides;
pub(crate) mod ownership;
mod pipeline;
mod request;
mod schedule;
mod shared_inputs;
mod toolchain;

pub use catalog::{EcosystemTasks, TaskCatalog, TaskSummary, task_catalog};
pub use configure::addressable_task_names;
pub use front::dependency_graph;
pub use host::PlanHost;
pub use pipeline::{FocusedPlan, plan, plan_focused};
pub use request::{CacheMode, PlanRequest, Selection};
pub use toven_model::ModuleSelector;

pub use front::{PlanContext, prepare as prepare_front};
pub use ownership::{AttributionPolicy, changed_records_for_module, changed_seeds};
