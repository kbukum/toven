//! [`HookInvocation`] — the valid inputs for one lifecycle hook invocation.

use std::path::Path;

use super::HookPhase;

/// A lifecycle hook invocation with any phase-specific payload encoded in its
/// variant.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum HookInvocation<'a> {
    /// Run before the unit's mutation.
    Before,
    /// Run after resolution with the authoritative version map.
    OnResolved {
        /// The materialized authoritative version-map file.
        version_map: &'a Path,
    },
    /// Run after the unit's mutation succeeds.
    After,
}

impl<'a> HookInvocation<'a> {
    /// The lifecycle phase represented by this invocation.
    #[must_use]
    pub const fn phase(self) -> HookPhase {
        match self {
            Self::Before => HookPhase::Before,
            Self::OnResolved { .. } => HookPhase::OnResolved,
            Self::After => HookPhase::After,
        }
    }

    /// The version map carried by an on-resolved invocation.
    #[must_use]
    pub const fn version_map(self) -> Option<&'a Path> {
        match self {
            Self::OnResolved { version_map } => Some(version_map),
            Self::Before | Self::After => None,
        }
    }
}
