//! Load-time task-name / reserved-word collision detection (cli-taxonomy Decision B).
//!
//! Argv-first dispatch shadows a task whose name equals a reserved built-in word:
//! `toven <name>` runs the built-in, and the task is reachable only via `toven run
//! <name>`. This module turns that shadow into a visible **warning** at config
//! load — never a hard stop (a later-reserved word must not break an existing
//! config) — pointing at the `toven run <task>` escape hatch.

use crate::grammar::is_reserved;

/// A single collision warning: a user-addressable task name equals a reserved word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The shadowed task name.
    pub task: String,
}

impl Collision {
    /// Render the warning line shown to the user.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "warning: task `{task}` shadows the reserved `toven {task}` built-in; run it with `toven run {task}`",
            task = self.task
        )
    }
}

/// Detect collisions for the user-addressable `task_names`.
///
/// Only genuinely user-named tasks should be passed (custom kinds and named
/// extras); the canonical built-in kinds (whose `run` overlap is by design) are
/// not collisions. Names are de-duplicated so a task discovered across several
/// ecosystems warns once.
pub fn detect<'a, I>(task_names: I) -> Vec<Collision>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut seen = std::collections::BTreeSet::new();
    task_names
        .into_iter()
        .filter(|name| is_reserved(name))
        .filter(|name| seen.insert(name.to_string()))
        .map(|name| Collision {
            task: name.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::detect;

    #[test]
    fn flags_a_task_named_like_a_reserved_word() {
        let collisions = detect(["test", "graph", "build"]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].task, "graph");
        assert!(collisions[0].message().contains("toven run graph"));
    }

    #[test]
    fn deduplicates_a_name_seen_across_ecosystems() {
        let collisions = detect(["release", "release"]);
        assert_eq!(collisions.len(), 1);
    }

    #[test]
    fn ordinary_task_names_do_not_collide() {
        assert!(detect(["test", "lint", "bench"]).is_empty());
    }
}
