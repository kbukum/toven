//! `toven-js` — the JavaScript/TypeScript ecosystem adapter.
//!
//! It depends only on [`toven_ports`] + [`toven_model`] (never the engine) and
//! is registered by id `"js"`:
//!
//! - [`JsProvider`] parses `[ecosystems.js]` into a typed [`JsConfig`] and
//!   bakes a [`JsAdapter`].
//! - [`JsAdapter`] implements offline discovery, the per-workspace toolchain
//!   probes, and run-strategy defaults. The runnable task table lives in
//!   `common().tasks`.
//!
//! Discovery is offline and in-process. It finds every `package.json` root
//! (one repository can hold several independent roots, each with its own
//! package manager), reads npm/Yarn `workspaces` and `pnpm-workspace.yaml`
//! member patterns, picks each root's package manager from the
//! `packageManager` pin, `devEngines`, the lockfile, or config, and links
//! packages through `workspace:`, path, and same-root range dependencies. It
//! never needs `node_modules`, the network, or a package manager binary, and
//! never spawns a process. All work returns typed data and typed errors.

// Internal helpers are shared across sibling modules as `pub(crate)`; see the
// same allowance in `toven-go` for why the nursery lint conflicts with
// `unreachable_pub` here.
#![allow(clippy::redundant_pub_crate)]

mod adapter;
mod config;
mod detect;
mod discovery;
mod doctor;
mod json_edit;
mod manifest;
mod package_manager;
mod patterns;
mod provider;
mod questionnaire;
mod range;
mod release;
mod render;
mod roots;
mod tasks;
mod test_runner;
mod toolchain;
mod walk;

pub use adapter::JsAdapter;
pub use config::{JsConfig, Roots};
pub use package_manager::Tool;
pub use provider::JsProvider;
