//! Shared helpers for adapter-provided defaults.

use crate::core::{AdapterId, PersistentReadiness, Task, TaskCommand, TaskOrigin};

/// Build a non-persistent argv task owned by an adapter default.
#[must_use]
pub fn argv_task(adapter_id: AdapterId, name: impl Into<String>, argv: Vec<String>) -> Task {
    Task {
        name: name.into(),
        command: TaskCommand::Argv(argv),
        origin: TaskOrigin::AdapterDefault { adapter_id },
        cache_args: false,
        shared_inputs: Vec::new(),
        persistent: false,
        readiness: PersistentReadiness::Started,
        readiness_timeout: std::time::Duration::from_secs(0),
    }
}
