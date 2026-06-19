//! The resolved task: a two-template command plus its scheduling attributes.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{FanOut, Readiness, TaskKind, TaskOrigin, readiness::DEFAULT_READINESS_TIMEOUT};

/// A fully resolved task — the adapter default field-merged with any user
/// override.
///
/// Carries the **two-template** command (`argv` base + per-module `selector`,
/// spliced at the `{module.selector}` point — see
/// [`CommandTemplate`](crate::template::CommandTemplate)) plus the attributes the
/// engine needs to schedule, cache, and run it. Per-task `run_strategy` /
/// `resource_group` overrides are engine-schedule config resolved later, by the
/// strict config `Document`, not carried on the port `Task`.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct Task {
    /// Identity slot; `Custom(name)` for ad-hoc tasks.
    pub kind: TaskKind,
    /// `Some` only for named extras within a kind (e.g. `test-integration`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Base argv template, rendered once (adapter default, user-overridable).
    pub argv: Vec<String>,
    /// Per-module fan-out fragment, spliced at `{module.selector}`.
    #[serde(default)]
    pub selector: Vec<String>,
    /// Capability ceiling; adapter default per kind.
    pub fan_out: FanOut,
    /// Where this resolved task came from.
    #[serde(default)]
    pub origin: TaskOrigin,
    /// Whether rendered passthrough args enter the cache key (default off).
    #[serde(default)]
    pub cache_args: bool,
    /// Task-level extra cache inputs (workspace-level lives on the adapter default).
    #[serde(default)]
    pub shared_inputs: Vec<String>,
    /// Orthogonal persistence flag; the `Run` kind defaults this true.
    #[serde(default)]
    pub persistent: bool,
    /// Readiness signal for persistent tasks.
    #[serde(default)]
    pub readiness: Readiness,
    /// Bound on how long to wait for readiness.
    #[serde(default = "default_readiness_timeout")]
    pub readiness_timeout: Duration,
}

const fn default_readiness_timeout() -> Duration {
    DEFAULT_READINESS_TIMEOUT
}

impl Task {
    /// Construct a task with the required identity + base argv and sensible
    /// defaults (non-persistent, [`TaskOrigin::AdapterDefault`], empty selector).
    /// Adapters set the remaining fields directly.
    #[must_use]
    pub const fn new(kind: TaskKind, argv: Vec<String>, fan_out: FanOut) -> Self {
        Self {
            kind,
            name: None,
            argv,
            selector: Vec::new(),
            fan_out,
            origin: TaskOrigin::AdapterDefault,
            cache_args: false,
            shared_inputs: Vec::new(),
            persistent: false,
            readiness: Readiness::Started,
            readiness_timeout: DEFAULT_READINESS_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_READINESS_TIMEOUT, FanOut, Readiness, Task, TaskKind, TaskOrigin};

    #[test]
    fn new_applies_sensible_defaults() {
        let task = Task::new(
            TaskKind::Build,
            vec!["cargo".into(), "build".into()],
            FanOut::WholeWorkspace,
        );
        assert_eq!(task.kind, TaskKind::Build);
        assert_eq!(task.argv, vec!["cargo".to_string(), "build".to_string()]);
        assert_eq!(task.fan_out, FanOut::WholeWorkspace);
        assert_eq!(task.origin, TaskOrigin::AdapterDefault);
        assert_eq!(task.readiness, Readiness::Started);
        assert_eq!(task.readiness_timeout, DEFAULT_READINESS_TIMEOUT);
        assert!(task.name.is_none());
        assert!(task.selector.is_empty());
        assert!(!task.cache_args);
        assert!(task.shared_inputs.is_empty());
        assert!(!task.persistent);
    }

    #[test]
    fn round_trips_through_toml() {
        let task = Task::new(
            TaskKind::Test,
            vec!["cargo".into(), "test".into()],
            FanOut::PerModule,
        );
        let json = toml::to_string(&task).expect("serialize");
        let back: Task = toml::from_str(&json).expect("deserialize");
        assert_eq!(task, back);
    }
}
