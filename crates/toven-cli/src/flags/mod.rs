//! The clap surface: global flags, the reserved-verb command tree, and the
//! per-verb applicability gate.
//!
//! Behavior-shaping flags are defined **once, globally** and accepted before or
//! after the verb; [`gate`] rejects a verb-specific flag used with a verb it
//! does not apply to with a clear, typed error. The execution flags a bare task
//! also accepts live in one clap group, [`ExecutionArgs`], which
//! [`grammar`](crate::grammar) reuses to parse a bare task's flag prefix, so
//! both dispatch routes share one schema.

mod cli;
mod execution;
mod gate;
mod values;

#[cfg(test)]
mod tests;

pub use cli::{CacheAction, Cli, Command, DriverAction, FederationAction, ReleaseAction};
pub use execution::ExecutionArgs;
pub use gate::gate;
pub use values::{
    ColorWhen, DEFAULT_WATCH_DEBOUNCE_MS, EnforcementArg, GraphFormat, OutputKind, Verbosity,
    ViewMode,
};
