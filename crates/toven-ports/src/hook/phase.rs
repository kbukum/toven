//! [`HookPhase`] — which side of a verb's mutation a hook runs on.

/// Which side of a verb's mutation a lifecycle hook runs on.
///
/// A [`Pre`](HookPhase::Pre) hook runs before the mutation and fails the verb
/// closed on failure (nothing is mutated); a [`Post`](HookPhase::Post) hook runs
/// after the mutation has succeeded.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HookPhase {
    /// Before the verb's mutation.
    Pre,
    /// After the verb's mutation succeeds.
    Post,
}

impl HookPhase {
    /// The stable lowercase label used in diagnostics and config (`pre`/`post`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pre => "pre",
            Self::Post => "post",
        }
    }
}
