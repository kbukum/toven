//! [`HookPhase`] — which lifecycle point of a unit's mutation a hook runs on.

/// Which lifecycle point of a unit's mutation a hook runs on.
///
/// A hook wraps **any** unit — a task, a native capability (bump/tag/publish),
/// or a composite — through one mechanism, differing only in phase:
/// - [`Before`](HookPhase::Before) runs before the unit's mutation and fails the
///   unit closed on failure (nothing is mutated);
/// - [`OnResolved`](HookPhase::OnResolved) runs *inside* the mutation once the
///   unit's decision is resolved but before it is staged — the bump seam handed
///   the authoritative post-bump version map;
/// - [`After`](HookPhase::After) runs after the mutation has succeeded.
///
/// `#[non_exhaustive]` because further lifecycle points may be added.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum HookPhase {
    /// Before the unit's mutation.
    Before,
    /// Inside the mutation, once the unit's decision is resolved but before it
    /// is staged.
    OnResolved,
    /// After the unit's mutation succeeds.
    After,
}

impl HookPhase {
    /// The stable lowercase label used in diagnostics
    /// (`before`/`on-resolved`/`after`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::OnResolved => "on-resolved",
            Self::After => "after",
        }
    }
}
