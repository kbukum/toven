//! `TaskEntry` — the complete config projection of a [`Task`], the authoritative
//! source of an ecosystem's runnable tasks.

use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::task::{FanOut, Readiness, Task, TaskKind, TaskOrigin};

/// One `[ecosystems.<id>.tasks.<name>]` entry: a complete, authored task.
///
/// Since [`toven init`](crate::provider::Provider::render) writes the full task
/// table into `toven.toml`, the config — not a compiled-in adapter default — is
/// the authoritative source of tasks. Each entry carries the two-template
/// command (`argv` + `selector`) plus the scheduling attributes the engine needs.
/// Its [`kind`](Self::kind) is derived from the table key for built-in names
/// (`test`, `build`, …) and treated as [`Custom`](TaskKind::Custom) otherwise; an
/// explicit `kind` marks a named extra within a built-in kind (e.g. a
/// `test-integration` entry with `kind = "test"`).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEntry {
    /// Explicit classifier for a named extra; omitted for a plain built-in or
    /// custom task, where the kind is derived from the table key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TaskKind>,
    /// The base argv template, rendered once per unit. Must be non-empty.
    pub argv: Vec<String>,
    /// The per-module selector fragment, spliced at `{module.selector}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selector: Vec<String>,
    /// The intrinsic fan-out ceiling.
    #[serde(default = "default_fan_out")]
    pub fan_out: FanOut,
    /// Whether this is a persistent (long-lived) task.
    #[serde(default, skip_serializing_if = "is_false")]
    pub persistent: bool,
    /// The readiness signal for a persistent task.
    #[serde(default, skip_serializing_if = "Readiness::is_started")]
    pub readiness: Readiness,
    /// The readiness timeout, in whole seconds (default 30s when omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_timeout_secs: Option<u64>,
    /// Whether rendered passthrough args enter the cache key.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cache_args: bool,
    /// Task-level extra cache inputs (workspace-relative plain paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_inputs: Vec<String>,
}

/// The default fan-out for a task entry when `fan_out` is omitted.
const fn default_fan_out() -> FanOut {
    FanOut::PerModule
}

/// Serde skip helper for a `false`-valued boolean.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

impl TaskEntry {
    /// The [`TaskKind`] and user-addressable name this entry resolves to under
    /// `key`: an explicit [`kind`](Self::kind) marks a named extra (name = `key`),
    /// otherwise the kind is derived from `key` (built-in or
    /// [`Custom`](TaskKind::Custom)) with no separate name.
    #[must_use]
    pub fn kind_and_name(&self, key: &str) -> (TaskKind, Option<String>) {
        self.kind.as_ref().map_or_else(
            || {
                (
                    TaskKind::builtin(key).unwrap_or_else(|| TaskKind::Custom(key.to_string())),
                    None,
                )
            },
            |kind| (kind.clone(), Some(key.to_string())),
        )
    }

    /// Materialize this config entry (under table key `key`) into a resolved
    /// [`Task`] with [`TaskOrigin::Project`].
    ///
    /// # Errors
    /// Returns a typed error citing `ecosystems.<ecosystem>.tasks.<key>` when
    /// `argv` is empty — the one completeness check the authoritative config must
    /// satisfy (a task cannot run without a command).
    pub fn materialize(&self, ecosystem: &str, key: &str) -> AppResult<Task> {
        if self.argv.is_empty() {
            return Err(AppError::invalid_input(
                format!("ecosystems.{ecosystem}.tasks.{key}"),
                "a task entry must define a non-empty 'argv'",
            ));
        }

        let (kind, name) = self.kind_and_name(key);
        let mut task = Task::new(kind, self.argv.clone(), self.fan_out);
        task.name = name;
        task.origin = TaskOrigin::Project;
        task.selector.clone_from(&self.selector);
        task.cache_args = self.cache_args;
        task.shared_inputs.clone_from(&self.shared_inputs);
        task.persistent = self.persistent;
        task.readiness = self.readiness.clone();
        if let Some(secs) = self.readiness_timeout_secs {
            task.readiness_timeout = Duration::from_secs(secs);
        }
        Ok(task)
    }
}

impl Readiness {
    /// Whether this is the default [`Started`](Readiness::Started) signal (so it
    /// can be skipped on serialize).
    #[must_use]
    pub const fn is_started(&self) -> bool {
        matches!(self, Self::Started)
    }
}

#[cfg(test)]
mod tests {
    use super::{FanOut, TaskEntry, TaskKind, TaskOrigin};

    fn entry(argv: &[&str]) -> TaskEntry {
        TaskEntry {
            kind: None,
            argv: argv.iter().map(ToString::to_string).collect(),
            selector: Vec::new(),
            fan_out: FanOut::PerModule,
            persistent: false,
            readiness: super::Readiness::Started,
            readiness_timeout_secs: None,
            cache_args: false,
            shared_inputs: Vec::new(),
        }
    }

    #[test]
    fn builtin_key_derives_kind_without_name() {
        let task = entry(&["cargo", "test"])
            .materialize("rust", "test")
            .expect("materializes");
        assert_eq!(task.kind, TaskKind::Test);
        assert!(task.name.is_none());
        assert_eq!(task.origin, TaskOrigin::Project);
    }

    #[test]
    fn unknown_key_derives_custom_kind() {
        let task = entry(&["cargo", "bench"])
            .materialize("rust", "bench")
            .expect("materializes");
        assert_eq!(task.kind, TaskKind::Custom("bench".into()));
        assert!(task.name.is_none());
    }

    #[test]
    fn explicit_kind_marks_a_named_extra() {
        let mut entry = entry(&["cargo", "test", "--test", "it"]);
        entry.kind = Some(TaskKind::Test);
        let task = entry
            .materialize("rust", "test-integration")
            .expect("materializes");
        assert_eq!(task.kind, TaskKind::Test);
        assert_eq!(task.name.as_deref(), Some("test-integration"));
    }

    #[test]
    fn empty_argv_is_rejected_with_a_pathful_message() {
        let error = entry(&[])
            .materialize("rust", "test")
            .expect_err("empty argv rejected");
        assert!(
            error.to_string().contains("ecosystems.rust.tasks.test"),
            "{error}"
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let mut entry = entry(&["cargo", "build"]);
        entry.fan_out = FanOut::Batchable;
        entry.shared_inputs = vec!["Cargo.lock".into()];
        let serialized = toml::to_string(&entry).expect("serialize");
        let back: TaskEntry = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(entry, back);
    }
}
