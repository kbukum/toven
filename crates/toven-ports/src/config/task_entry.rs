//! `TaskEntry` — the complete config projection of a [`Task`], the
//! authoritative source of an ecosystem's runnable tasks.

use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::task::{FanOut, Readiness, Task, TaskKind, TaskOrigin};

/// One `[ecosystems.<id>.tasks.<name>]` entry: a complete, authored task.
///
/// Since [`toven init`](crate::provider::Provider::render) writes the full task
/// table into `toven.toml`, the config — not a compiled-in adapter default — is
/// the authoritative source of tasks. The table key is the task's identity (the
/// name a user types); each entry carries the two-template command (`argv` +
/// `selector`) plus the scheduling attributes the engine needs. Its
/// [`kind`](Self::kind) is an optional recognition attribute: when omitted it
/// defaults to the recognized kind matching the key (`test` → `Test`), and it
/// can be set explicitly to preserve recognition across a rename (e.g. a
/// `my-test` entry with `kind = "test"`).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // a task schema is a set of independent flags
pub struct TaskEntry {
    /// Optional recognition attribute; when omitted it defaults to the
    /// recognized kind matching the table key, else [`TaskKind::Default`].
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
    /// Whether this task's result may be cached. Defaults to `true`; a
    /// tree-mutating task (a `*-fix` twin, e.g. `gofmt -w` or `go mod tidy`)
    /// authors `cacheable = false` so a stale content-key hit never suppresses
    /// the mutation on a later run.
    #[serde(default = "default_cacheable", skip_serializing_if = "is_true")]
    pub cacheable: bool,
    /// Whether any stdout output turns a zero-exit run into a failure. Defaults
    /// to `false`. A list-mode verification whose tool reports offenders on
    /// stdout but still exits `0` (e.g. `gofmt -l`) authors `fail_if_output =
    /// true` so it becomes a real CI gate instead of a silent pass.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fail_if_output: bool,
    /// Task-level extra cache inputs (workspace-relative plain paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_inputs: Vec<String>,
}

/// The default fan-out for a task entry when `fan_out` is omitted.
const fn default_fan_out() -> FanOut {
    FanOut::PerModule
}

/// The default `cacheable` for a task entry when the field is omitted.
const fn default_cacheable() -> bool {
    true
}

/// Serde skip helper for a `false`-valued boolean.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Serde skip helper for a `true`-valued boolean (omit the on-by-default flag).
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(value: &bool) -> bool {
    *value
}

impl TaskEntry {
    /// The recognized [`TaskKind`] this entry resolves to under `key`: the
    /// explicit [`kind`](Self::kind) when set, else the recognized kind
    /// matching `key`, else [`TaskKind::Default`].
    #[must_use]
    pub fn resolved_kind(&self, key: &str) -> TaskKind {
        self.kind
            .or_else(|| TaskKind::from_name(key))
            .unwrap_or(TaskKind::Default)
    }

    /// Materialize this config entry (under table key `key`) into a resolved
    /// [`Task`] with [`TaskOrigin::Project`]. The `key` becomes the task's name
    /// identity; its recognized kind is [`resolved_kind`](Self::resolved_kind).
    ///
    /// # Errors
    /// Returns a typed error citing `ecosystems.<ecosystem>.tasks.<key>` when
    /// `argv` is empty — the one completeness check the authoritative config
    /// must satisfy (a task cannot run without a command).
    pub fn materialize(&self, ecosystem: &str, key: &str) -> AppResult<Task> {
        if self.argv.is_empty() {
            return Err(AppError::invalid_input(
                format!("ecosystems.{ecosystem}.tasks.{key}"),
                "a task entry must define a non-empty 'argv'",
            ));
        }

        let mut task =
            Task::new(key, self.argv.clone(), self.fan_out).with_kind(self.resolved_kind(key));
        task.origin = TaskOrigin::Project;
        task.selector.clone_from(&self.selector);
        task.cache_args = self.cache_args;
        task.cacheable = self.cacheable;
        task.fail_if_output = self.fail_if_output;
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
    /// Whether this is the default [`Started`](Readiness::Started) signal (so
    /// it can be skipped on serialize).
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
            cacheable: true,
            fail_if_output: false,
            shared_inputs: Vec::new(),
        }
    }

    #[test]
    fn builtin_key_derives_kind_and_name() {
        let task = entry(&["cargo", "test"])
            .materialize("rust", "test")
            .expect("materializes");
        assert_eq!(task.name, "test");
        assert_eq!(task.kind, TaskKind::Test);
        assert_eq!(task.origin, TaskOrigin::Project);
    }

    #[test]
    fn unrecognized_key_defaults_kind() {
        let task = entry(&["cargo", "bench"])
            .materialize("rust", "bench")
            .expect("materializes");
        assert_eq!(task.name, "bench");
        assert_eq!(task.kind, TaskKind::Default);
    }

    #[test]
    fn explicit_kind_preserves_recognition_across_rename() {
        let mut entry = entry(&["cargo", "test", "--test", "it"]);
        entry.kind = Some(TaskKind::Test);
        let task = entry.materialize("rust", "my-test").expect("materializes");
        assert_eq!(task.name, "my-test");
        assert_eq!(task.kind, TaskKind::Test);
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

    #[test]
    fn cacheable_defaults_true_and_is_omitted_when_on() {
        let entry = entry(&["gofmt", "-l", "."]);
        assert!(entry.cacheable);
        let serialized = toml::to_string(&entry).expect("serialize");
        assert!(
            !serialized.contains("cacheable"),
            "the on-by-default flag is omitted: {serialized}"
        );
    }

    #[test]
    fn cacheable_false_survives_a_round_trip_and_materializes() {
        let mut entry = entry(&["gofmt", "-w", "."]);
        entry.cacheable = false;
        let serialized = toml::to_string(&entry).expect("serialize");
        assert!(serialized.contains("cacheable = false"), "{serialized}");
        let back: TaskEntry = toml::from_str(&serialized).expect("deserialize");
        assert!(!back.cacheable);
        let task = back.materialize("go", "format-fix").expect("materializes");
        assert!(!task.cacheable);
    }

    #[test]
    fn fail_if_output_defaults_false_and_is_omitted_when_off() {
        let entry = entry(&["gofmt", "-l", "."]);
        assert!(!entry.fail_if_output);
        let serialized = toml::to_string(&entry).expect("serialize");
        assert!(
            !serialized.contains("fail_if_output"),
            "the off-by-default flag is omitted: {serialized}"
        );
    }

    #[test]
    fn fail_if_output_true_survives_a_round_trip_and_materializes() {
        let mut entry = entry(&["gofmt", "-l", "."]);
        entry.fail_if_output = true;
        let serialized = toml::to_string(&entry).expect("serialize");
        assert!(serialized.contains("fail_if_output = true"), "{serialized}");
        let back: TaskEntry = toml::from_str(&serialized).expect("deserialize");
        assert!(back.fail_if_output);
        let task = back
            .materialize("go", "format-check")
            .expect("materializes");
        assert!(task.fail_if_output);
    }
}
