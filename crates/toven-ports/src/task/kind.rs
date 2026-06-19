//! Canonical task kind — the identity that drives CLI verbs and lifecycle order.

use serde::{Deserialize, Serialize};

/// Canonical, closed task vocabulary.
///
/// The **kind** is a task's identity: it drives CLI verbs
/// (`toven test`), lifecycle ordering (fmt → lint, build before test), and
/// cross-ecosystem semantics ("run `test` everywhere"). [`Custom`](TaskKind::Custom)
/// is the escape hatch for genuinely ad-hoc tasks.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskKind {
    /// Compile the project.
    Build,
    /// Type-check / fast verify without producing artifacts.
    Check,
    /// Format sources.
    Format,
    /// Lint sources.
    Lint,
    /// Run the test suite.
    Test,
    /// Build documentation.
    Doc,
    /// Persistent / dev run (servers, watchers); adapter defaults are persistent.
    Run,
    /// Genuinely ad-hoc task addressed by name.
    Custom(String),
}

impl TaskKind {
    /// Resolve a built-in kind from its canonical lowercase name.
    ///
    /// Returns `None` for names that are not built-in kinds; the caller treats
    /// those as [`Custom`](TaskKind::Custom).
    #[must_use]
    pub fn builtin(name: &str) -> Option<Self> {
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

    /// Canonical lowercase name of this kind (the `Custom` payload for custom tasks).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Build => "build",
            Self::Check => "check",
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Test => "test",
            Self::Doc => "doc",
            Self::Run => "run",
            Self::Custom(name) => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskKind;

    #[test]
    fn builtin_round_trips_by_name() {
        for kind in [
            TaskKind::Build,
            TaskKind::Check,
            TaskKind::Format,
            TaskKind::Lint,
            TaskKind::Test,
            TaskKind::Doc,
            TaskKind::Run,
        ] {
            assert_eq!(TaskKind::builtin(kind.name()), Some(kind));
        }
    }

    #[test]
    fn unknown_name_is_not_builtin() {
        assert_eq!(TaskKind::builtin("bench"), None);
    }
}
