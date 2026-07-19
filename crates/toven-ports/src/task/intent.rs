//! `TaskIntent` — the task a PLAN run targets: a name plus its recognized kind.

use super::TaskKind;

/// What a PLAN run targets: the user-typed task **name** (the identity that
/// selects which config task runs) paired with its recognized [`TaskKind`].
///
/// The name is authoritative — `toven <name>` resolves the config task whose
/// addressable name equals it. The kind is the recognition attribute: initially
/// derived from the name (`test` → [`TaskKind::Test`]), it is superseded during
/// planning by the addressed task's configured `kind` so a renamed task
/// (`my-test` with `kind = "test"`) still drives kind-aware behavior such as
/// the dev-edge rule in affected-set selection; an unrecognized name resolves
/// to [`TaskKind::Default`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TaskIntent {
    /// The canonical task name the user typed (the identity).
    name: String,
    /// The recognized kind derived from the name.
    kind: TaskKind,
}

impl TaskIntent {
    /// Resolve an intent from a task `name`, deriving its recognized kind.
    #[must_use]
    pub fn resolve(name: impl Into<String>) -> Self {
        let name = name.into();
        let kind = TaskKind::from_name(&name).unwrap_or(TaskKind::Default);
        Self { name, kind }
    }

    /// Override the recognized kind (the addressed config task's `kind`
    /// attribute supersedes the name-derived default so recognition survives a
    /// rename).
    #[must_use]
    pub const fn with_kind(mut self, kind: TaskKind) -> Self {
        self.kind = kind;
        self
    }

    /// The canonical task name (the identity a config task is matched against).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The recognized kind derived from the name.
    #[must_use]
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }
}
