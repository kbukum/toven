//! [`TaskDependency`] — a task that must finish before another task runs.

use std::fmt;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Which modules a prerequisite task runs on, relative to the module that
/// needs it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DependencyScope {
    /// The same module (`"build"`).
    Module,
    /// Every module the module depends on, transitively through the
    /// prerequisite's own `depends_on` (`"^build"`).
    Dependencies,
}

/// One `depends_on` entry: `"build"` runs `build` on the same module first;
/// `"^build"` runs `build` on the module's dependencies first.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskDependency {
    task: String,
    scope: DependencyScope,
}

/// The prefix that marks a dependency-scoped prerequisite.
const DEPENDENCIES_PREFIX: char = '^';

impl TaskDependency {
    /// Parse one `depends_on` entry.
    ///
    /// # Errors
    /// Returns [`AppError::invalid_input`] when the task name is empty or
    /// contains whitespace or another `^`.
    pub fn parse(value: &str) -> AppResult<Self> {
        let (scope, task) = value
            .strip_prefix(DEPENDENCIES_PREFIX)
            .map_or((DependencyScope::Module, value), |task| {
                (DependencyScope::Dependencies, task)
            });
        if task.is_empty()
            || task.contains(DEPENDENCIES_PREFIX)
            || task.chars().any(char::is_whitespace)
        {
            return Err(AppError::invalid_input(
                "depends_on",
                format!(
                    "'{value}' is not a task prerequisite; use a task name ('build') or '^' and a task name ('^build')"
                ),
            ));
        }
        Ok(Self {
            task: task.to_string(),
            scope,
        })
    }

    /// The prerequisite task's name.
    #[must_use]
    pub fn task(&self) -> &str {
        &self.task
    }

    /// Which modules the prerequisite runs on.
    #[must_use]
    pub const fn scope(&self) -> DependencyScope {
        self.scope
    }
}

impl fmt::Display for TaskDependency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scope {
            DependencyScope::Module => formatter.write_str(&self.task),
            DependencyScope::Dependencies => {
                write!(formatter, "{DEPENDENCIES_PREFIX}{}", self.task)
            }
        }
    }
}

impl TryFrom<String> for TaskDependency {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TaskDependency> for String {
    fn from(value: TaskDependency) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{DependencyScope, TaskDependency};

    #[test]
    fn parses_both_scopes_and_round_trips() {
        let same = TaskDependency::parse("build").unwrap();
        assert_eq!(same.task(), "build");
        assert_eq!(same.scope(), DependencyScope::Module);
        assert_eq!(same.to_string(), "build");

        let deps = TaskDependency::parse("^build").unwrap();
        assert_eq!(deps.task(), "build");
        assert_eq!(deps.scope(), DependencyScope::Dependencies);
        assert_eq!(deps.to_string(), "^build");
    }

    #[test]
    fn rejects_malformed_entries() {
        for bad in ["", "^", "^^build", "my build", "a^b"] {
            assert!(
                TaskDependency::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn deserializes_from_a_string_list() {
        #[derive(serde::Deserialize)]
        struct Doc {
            depends_on: Vec<TaskDependency>,
        }
        let doc: Doc = toml::from_str(r#"depends_on = ["build", "^build"]"#).unwrap();
        assert_eq!(doc.depends_on.len(), 2);
        assert!(toml::from_str::<Doc>(r#"depends_on = ["^"]"#).is_err());
    }
}
