//! `TaskOverride` — the user's per-task diff, field-merged over an adapter
//! default.

use serde::{Deserialize, Serialize};

use crate::task::{FanOut, Readiness, TaskKind};

/// A user override for a single **group** task (`[groups.<name>].tasks`).
///
/// This is the sparse group-layer diff only: the ecosystem-level task table is
/// the complete [`TaskEntry`](crate::config::TaskEntry) shape, not this. Every
/// field is optional: an unset field inherits the config base task during
/// field-merge ([`merge_task`](crate::merge::merge_task)). Scalars and lists
/// **replace**; `shared_inputs` is the one **additive** list (it extends the
/// cache-key footprint).
///
/// Only fields that exist on the resolved [`Task`](crate::task::Task) live
/// here. Per-task engine-schedule knobs (`run_strategy`, `resource_group`) are
/// resolved by the strict config `Document`, not by this port-level merge.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOverride {
    /// Recognition attribute for the task; ignored when the table key already
    /// matches a recognized kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TaskKind>,
    /// Replacement base argv template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Replacement per-module selector fragment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<Vec<String>>,
    /// Replacement `fan_out` ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_out: Option<FanOut>,
    /// Replacement `workspace_closure` capability flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_closure: Option<bool>,
    /// Replacement persistence flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent: Option<bool>,
    /// Replacement readiness signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<Readiness>,
    /// Replacement readiness timeout, in whole seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_timeout_secs: Option<u64>,
    /// Replacement `cache_args` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_args: Option<bool>,
    /// Replacement `cacheable` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cacheable: Option<bool>,
    /// Replacement `fail_if_output` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_if_output: Option<bool>,
    /// Extra task-level cache inputs, **unioned** with the adapter default set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_inputs: Vec<String>,
}
