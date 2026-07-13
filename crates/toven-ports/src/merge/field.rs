//! Field-merge: resolve an adapter default [`Task`] against a user [`TaskOverride`].

use std::collections::HashSet;
use std::time::Duration;

use crate::{
    config::TaskOverride,
    task::{Task, TaskOrigin},
};

/// Field-merge a user override onto an adapter default task.
///
/// Set scalar/list fields in the override **replace** the default; `shared_inputs`
/// is **unioned** (preserving order, de-duplicated) with the default's set. An
/// unset override field inherits the default — so a bare `cache_args = true`
/// flips exactly one field while `selector`, `fan_out`, and the rest carry over.
/// The result's [`origin`](Task::origin) becomes [`TaskOrigin::Project`].
///
/// The default's `name` (its identity) and `kind` carry over unchanged:
/// [`TaskOverride::kind`](crate::config::TaskOverride::kind) is a recognition
/// attribute the `Document` loader consumes when building a task that has no
/// matching default, not a field merged here.
#[must_use]
pub fn merge_task(default: &Task, over: &TaskOverride) -> Task {
    let mut merged = default.clone();
    merged.origin = TaskOrigin::Project;

    if let Some(argv) = &over.argv {
        merged.argv.clone_from(argv);
    }
    if let Some(selector) = &over.selector {
        merged.selector.clone_from(selector);
    }
    if let Some(fan_out) = over.fan_out {
        merged.fan_out = fan_out;
    }
    if let Some(persistent) = over.persistent {
        merged.persistent = persistent;
    }
    if let Some(readiness) = &over.readiness {
        merged.readiness = readiness.clone();
    }
    if let Some(secs) = over.readiness_timeout_secs {
        merged.readiness_timeout = Duration::from_secs(secs);
    }
    if let Some(cache_args) = over.cache_args {
        merged.cache_args = cache_args;
    }
    if let Some(cacheable) = over.cacheable {
        merged.cacheable = cacheable;
    }
    if let Some(fail_if_output) = over.fail_if_output {
        merged.fail_if_output = fail_if_output;
    }
    union_in_place(&mut merged.shared_inputs, &over.shared_inputs);

    merged
}

/// Replace `base` with the de-duplicated union of `base` then `extra`,
/// preserving first-occurrence order across both.
///
/// Membership is tracked in a `HashSet` of borrowed `&str` so the union stays
/// linear in the total number of inputs. The result drops duplicates already
/// present within `base` as well as any `extra` entry that repeats one, so the
/// merged `shared_inputs` truly has no duplicates regardless of source. Each
/// kept entry is cloned exactly once into the rebuilt vector.
fn union_in_place(base: &mut Vec<String>, extra: &[String]) {
    let mut seen: HashSet<&str> = HashSet::with_capacity(base.len() + extra.len());
    let mut union: Vec<String> = Vec::with_capacity(base.len() + extra.len());
    for input in base.iter().chain(extra) {
        if seen.insert(input.as_str()) {
            union.push(input.clone());
        }
    }
    *base = union;
}

#[cfg(test)]
mod tests {
    use super::merge_task;
    use crate::{
        config::TaskOverride,
        task::{FanOut, Task, TaskOrigin},
    };

    fn default_test_task() -> Task {
        let mut task = Task::new(
            "test",
            vec!["cargo".into(), "test".into(), "{module.selector}".into()],
            FanOut::Batchable,
        );
        task.selector = vec!["-p".into(), "{module.package}".into()];
        task.shared_inputs = vec!["Cargo.lock".into()];
        task
    }

    #[test]
    fn override_replaces_argv_and_inherits_the_rest() {
        let over = TaskOverride {
            argv: Some(vec!["cargo".into(), "nextest".into(), "run".into()]),
            cache_args: Some(true),
            ..TaskOverride::default()
        };

        let merged = merge_task(&default_test_task(), &over);

        assert_eq!(merged.argv, ["cargo", "nextest", "run"]);
        assert!(merged.cache_args);
        // inherited, untouched by the override:
        assert_eq!(merged.selector, ["-p", "{module.package}"]);
        assert_eq!(merged.fan_out, FanOut::Batchable);
        assert_eq!(merged.shared_inputs, ["Cargo.lock"]);
        assert_eq!(merged.origin, TaskOrigin::Project);
    }

    #[test]
    fn shared_inputs_union_dedups_and_appends() {
        let over = TaskOverride {
            shared_inputs: vec!["Cargo.lock".into(), "build.rs".into()],
            ..TaskOverride::default()
        };

        let merged = merge_task(&default_test_task(), &over);

        assert_eq!(merged.shared_inputs, ["Cargo.lock", "build.rs"]);
    }

    #[test]
    fn shared_inputs_union_drops_duplicates_already_in_the_default() {
        let mut default = default_test_task();
        default.shared_inputs = vec!["Cargo.lock".into(), "Cargo.lock".into()];
        let over = TaskOverride {
            shared_inputs: vec!["Cargo.lock".into(), "build.rs".into()],
            ..TaskOverride::default()
        };

        let merged = merge_task(&default, &over);

        assert_eq!(merged.shared_inputs, ["Cargo.lock", "build.rs"]);
    }
}
