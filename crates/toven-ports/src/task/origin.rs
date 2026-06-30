//! Provenance of a resolved task.

use serde::{Deserialize, Serialize};

/// Where a resolved [`Task`](super::Task) came from.
///
/// Drives reporting and explains which layer won during field-merge.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum TaskOrigin {
    /// The adapter's built-in default for the kind.
    #[default]
    AdapterDefault,
    /// A project-level `[ecosystems.<id>.tasks.<name>]` override.
    Project,
}
