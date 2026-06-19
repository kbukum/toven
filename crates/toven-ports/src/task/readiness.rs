//! Readiness signalling for persistent tasks.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default readiness timeout for a persistent task.
pub const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// How the engine decides a persistent task has become ready.
///
/// Language-agnostic by design; the engine owns the lifecycle (spawn → gate
/// dependents on readiness → run dependents → teardown) and the adapter only
/// supplies the signal.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    /// Ready once the subprocess starts.
    #[default]
    Started,
    /// Ready when a bounded health command exits successfully.
    Command(Vec<String>),
    /// Ready when literal text appears on stdout/stderr.
    OutputContains(String),
}
