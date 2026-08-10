//! The exec adapters: concrete runners for the [`toven_ports`] execution seams.
//!
//! Layer 2a hosts the shared synchronous [`ProcessToolRunner`] here so every
//! downward crate (release, engine, cli) drives one-shot tools through one
//! `rskit-process`-backed runner rather than re-wiring the spawn/capture/timeout
//! policy per call site. The async streaming
//! [`CommandRunner`](toven_ports::CommandRunner) adapter lives in
//! `toven-engine` (it belongs to the APPLY wave walk).

mod tool;

pub use tool::ProcessToolRunner;
