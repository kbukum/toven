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
/// The default's `kind`/`name` (its slot identity) carry over unchanged:
/// [`TaskOverride::kind`](crate::config::TaskOverride::kind) is a classifier the
/// `Document` loader consumes when building named/custom extras that have no
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
    union_in_place(&mut merged.shared_inputs, &over.shared_inputs);

    merged
}

/// Append every entry of `extra` not already present, preserving order.
///
/// Membership is tracked in a `HashSet` of borrowed `&str` so the union stays
/// linear in the total number of inputs and each new entry is cloned exactly
/// once — only when it is actually appended to `base`.
fn union_in_place(base: &mut Vec<String>, extra: &[String]) {
    if extra.is_empty() {
        return;
    }
    let mut seen: HashSet<&str> = base.iter().map(String::as_str).collect();
    let additions: Vec<String> = extra
        .iter()
        .filter(|input| seen.insert(input.as_str()))
        .cloned()
        .collect();
    base.extend(additions);
}

#[cfg(test)]
mod tests {
    use super::merge_task;
    use crate::{
        config::TaskOverride,
        task::{FanOut, Task, TaskKind, TaskOrigin},
    };

    fn default_test_task() -> Task {
        let mut task = Task::new(
            TaskKind::Test,
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
}
