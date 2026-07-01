//! The pure PLAN spine: the seven phases (Load → Configure → Discover → Graph →
//! Affected → Toolchain → Schedule+Cache) culminating in one immutable
//! [`Plan`](toven_model::Plan).
//!
//! Load is the [`config`](crate::config) loader; the remaining phases
//! live one-per-file under this module and are driven by [`pipeline::plan`]. The
//! cross-language differentiator is built here: ONE federated dependency graph,
//! assembled centrally before affected/schedule.
//!
//! ## Surface
//! - `pipeline` — drives the phases and emits PHASE/PLAN events → [`Plan`](toven_model::Plan).
//! - `configure` — bakes each ecosystem subtree into a `ConfiguredAdapter`.
//! - `discover` — full federation union (workspaces/modules/edges + overlays).
//! - `graph` — `Graph::build` plus the deferred SEMANTIC config validation.
//! - `affected` — longest-prefix change mapper + blast radius + closure.
//! - `toolchain` — per-active-workspace `{tool, version}` resolution.
//! - `schedule` — `RunStrategy` relaxation → federated waves → per-module units.
//! - `cache` — the content key, lookup port, and per-unit verdict.
//! - `request` / `source` / `host` — PLAN inputs and injected ports.

pub(crate) mod affected;
mod cache;
pub(crate) mod configure;
pub(crate) mod discover;
pub(crate) mod front;
mod graph;
mod host;
mod pipeline;
mod request;
mod schedule;
mod shared_inputs;
mod source;
mod toolchain;

pub use cache::NullCache;
pub use configure::addressable_task_names;
pub use front::dependency_graph;
pub use host::PlanHost;
pub use pipeline::plan;
pub use request::{CacheMode, ModuleSelector, PlanRequest, Selection};
pub use source::FsSourceDigest;
pub use toolchain::ProcessToolchainProber;

pub(crate) use affected::{changed_records_for_module, changed_seeds};
pub(crate) use front::{PlanContext, prepare as prepare_front};
