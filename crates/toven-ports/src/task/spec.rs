//! The resolved task: a two-template command plus its scheduling attributes.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use toven_model::Unit;

use super::{FanOut, Readiness, TaskKind, TaskOrigin, readiness::DEFAULT_READINESS_TIMEOUT};

/// A fully resolved task — the adapter default field-merged with any user
/// override.
///
/// Carries the **two-template** command (`argv` base + per-module `selector`,
/// spliced at the `{module.selector}` point — see
/// [`CommandTemplate`](crate::template::CommandTemplate)) plus the attributes
/// the engine needs to schedule, cache, and run it. Its [`name`](Self::name) is
/// the identity a user types (`toven <name>`); [`kind`](Self::kind) is the
/// optional recognition attribute. Per-task `run_strategy` / `resource_group`
/// overrides are engine-schedule config resolved later, by the strict config
/// `Document`, not carried on the port `Task`.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[allow(clippy::struct_excessive_bools)] // a task schema is a set of independent flags
pub struct Task {
    /// The task's identity: the name a user types (`toven <name>`).
    pub name: String,
    /// The recognized kind of this task, or [`TaskKind::Default`] when the name
    /// matches no recognized kind.
    pub kind: TaskKind,
    /// Base argv template, rendered once (adapter default, user-overridable).
    pub argv: Vec<String>,
    /// Per-module fan-out fragment, spliced at `{module.selector}`.
    #[serde(default)]
    pub selector: Vec<String>,
    /// Capability ceiling; adapter default per kind.
    pub fan_out: FanOut,
    /// Whether this task's whole-workspace invocation resolves its own
    /// cross-workspace dependency closure, making it sound to co-schedule inside
    /// an irreducible facade back-dependency cycle (default off).
    ///
    /// Only honored with [`fan_out`](Self::fan_out) =
    /// [`WholeWorkspace`](FanOut::WholeWorkspace): it is the verified semantic —
    /// separate from the fan-out ceiling — that the scheduler consults before
    /// condensing a facade cycle into one co-scheduled wave. Adapters set it on
    /// tool invocations that operate on the whole workspace atomically (`cargo …
    /// --workspace`, `go … ./...`); it stays `false` for arbitrary custom
    /// commands so a cycle touching one keeps failing closed.
    #[serde(default)]
    pub workspace_closure: bool,
    /// Where this resolved task came from.
    #[serde(default)]
    pub origin: TaskOrigin,
    /// Whether rendered passthrough args enter the cache key (default off).
    #[serde(default)]
    pub cache_args: bool,
    /// Whether this task's result may be cached (default on). A tree-mutating
    /// task authors `false` so a stale content-key hit never suppresses the
    /// mutation on a later run.
    #[serde(default = "default_cacheable")]
    pub cacheable: bool,
    /// Whether any stdout output turns a zero-exit run into a failure (default
    /// off). A list-mode verification that reports offenders on stdout but
    /// exits `0` (e.g. `gofmt -l`) authors `true` so it gates instead of
    /// silently passing.
    #[serde(default)]
    pub fail_if_output: bool,
    /// Task-level extra cache inputs (workspace-level lives on the adapter
    /// default).
    #[serde(default)]
    pub shared_inputs: Vec<String>,
    /// Orthogonal persistence flag; the `Run` kind seeds this true at init.
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

/// The default `cacheable` for a task when the field is omitted.
const fn default_cacheable() -> bool {
    true
}

impl Task {
    /// Construct a task with the required `name` identity + base argv and
    /// sensible defaults (recognized kind derived from the name,
    /// non-persistent, [`TaskOrigin::AdapterDefault`], empty selector).
    /// Adapters set the remaining fields directly.
    #[must_use]
    pub fn new(name: impl Into<String>, argv: Vec<String>, fan_out: FanOut) -> Self {
        let name = name.into();
        let kind = TaskKind::from_name(&name).unwrap_or(TaskKind::Default);
        Self {
            name,
            kind,
            argv,
            selector: Vec::new(),
            fan_out,
            workspace_closure: false,
            origin: TaskOrigin::AdapterDefault,
            cache_args: false,
            cacheable: true,
            fail_if_output: false,
            shared_inputs: Vec::new(),
            persistent: false,
            readiness: Readiness::Started,
            readiness_timeout: DEFAULT_READINESS_TIMEOUT,
        }
    }

    /// Override the recognized [`kind`](Self::kind) (e.g. tag a renamed task so
    /// it keeps its kind-aware behavior).
    #[must_use]
    pub const fn with_kind(mut self, kind: TaskKind) -> Self {
        self.kind = kind;
        self
    }

    /// Express this task as a [`Unit`] with an argv backing, so a task and a
    /// native release capability speak the one unified vocabulary: a task's
    /// identity is its [`name`](Self::name) and its backing is always
    /// [`Argv`](toven_model::Backing::Argv).
    #[must_use]
    pub fn as_unit(&self) -> Unit {
        Unit::argv(self.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_READINESS_TIMEOUT, FanOut, Readiness, Task, TaskKind, TaskOrigin};

    #[test]
    fn new_applies_sensible_defaults() {
        let task = Task::new(
            "build",
            vec!["cargo".into(), "build".into()],
            FanOut::WholeWorkspace,
        );
        assert_eq!(task.name, "build");
        assert_eq!(task.kind, TaskKind::Build);
        assert_eq!(task.argv, vec!["cargo".to_string(), "build".to_string()]);
        assert_eq!(task.fan_out, FanOut::WholeWorkspace);
        assert_eq!(task.origin, TaskOrigin::AdapterDefault);
        assert_eq!(task.readiness, Readiness::Started);
        assert_eq!(task.readiness_timeout, DEFAULT_READINESS_TIMEOUT);
        assert!(task.selector.is_empty());
        assert!(!task.cache_args);
        assert!(task.cacheable);
        assert!(!task.fail_if_output);
        assert!(task.shared_inputs.is_empty());
        assert!(!task.persistent);
        assert!(!task.workspace_closure);
    }

    #[test]
    fn unrecognized_name_defaults_kind() {
        let task = Task::new(
            "bench",
            vec!["cargo".into(), "bench".into()],
            FanOut::PerModule,
        );
        assert_eq!(task.kind, TaskKind::Default);
    }

    #[test]
    fn with_kind_overrides_recognition() {
        let task = Task::new(
            "test-integration",
            vec!["cargo".into(), "test".into()],
            FanOut::PerModule,
        )
        .with_kind(TaskKind::Test);
        assert_eq!(task.name, "test-integration");
        assert_eq!(task.kind, TaskKind::Test);
    }

    #[test]
    fn expresses_as_an_argv_unit() {
        // Normalization: a task maps onto the unified `Unit` vocabulary as an
        // argv-backed unit named for the task identity.
        let task = Task::new(
            "build",
            vec!["cargo".into(), "build".into()],
            FanOut::WholeWorkspace,
        );
        let unit = task.as_unit();
        assert_eq!(unit.name(), "build");
        assert_eq!(unit.backing().as_str(), "argv");
    }

    #[test]
    fn round_trips_through_toml() {
        let task = Task::new(
            "test",
            vec!["cargo".into(), "test".into()],
            FanOut::PerModule,
        );
        let serialized = toml::to_string(&task).expect("serialize");
        let back: Task = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(task, back);
    }
}
