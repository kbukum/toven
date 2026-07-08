//! Task kind — an optional recognition attribute, not a task's identity.

use serde::{Deserialize, Serialize};

/// A recognized task kind: an optional attribute that grants a named task a few
/// kind-aware behaviors.
///
/// A task's **identity** is its name (`[ecosystems.<id>.tasks.<name>]`), not its
/// kind — `toven <name>` runs whatever the config defines, npm-scripts style.
/// `kind` is a recognition tag that survives a rename: tag a task `kind = "test"`
/// and it keeps the [`Test`](TaskKind::Test) dev-edge rule, cross-ecosystem
/// fan-out matching, and the kind-aware run-strategy default even if the user
/// renames it. A task with no recognized kind is [`Default`](TaskKind::Default) —
/// a plain named task with none of those behaviors.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum TaskKind {
    /// Compile the project.
    Build,
    /// Type-check / fast verify without producing artifacts.
    Check,
    /// Format sources.
    Format,
    /// Lint sources.
    Lint,
    /// Run the test suite (the one kind with a runtime rule: dev-edge propagation).
    Test,
    /// Build documentation.
    Doc,
    /// Persistent / dev run (servers, watchers); seeds a persistent default at init.
    Run,
    /// No recognized kind: a plain named task with no kind-aware behavior.
    Default,
}

impl TaskKind {
    /// Resolve a recognized kind from its canonical lowercase name.
    ///
    /// Returns `None` for names outside the recognized set (including
    /// `"default"`); the caller treats those as [`Default`](TaskKind::Default).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "build" => Self::Build,
            "check" => Self::Check,
            "format" => Self::Format,
            "lint" => Self::Lint,
            "test" => Self::Test,
            "doc" => Self::Doc,
            "run" => Self::Run,
            _ => return None,
        })
    }

    /// The canonical lowercase name of this kind.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Check => "check",
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Test => "test",
            Self::Doc => "doc",
            Self::Run => "run",
            Self::Default => "default",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskKind;

    #[test]
    fn recognized_round_trips_by_name() {
        for kind in [
            TaskKind::Build,
            TaskKind::Check,
            TaskKind::Format,
            TaskKind::Lint,
            TaskKind::Test,
            TaskKind::Doc,
            TaskKind::Run,
        ] {
            assert_eq!(TaskKind::from_name(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn unrecognized_name_is_not_a_kind() {
        assert_eq!(TaskKind::from_name("bench"), None);
        assert_eq!(TaskKind::from_name("default"), None);
    }
}
