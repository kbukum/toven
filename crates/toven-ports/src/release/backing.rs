//! [`PhaseBacking`] — how a single release phase is satisfied.

/// How a single release [`ReleasePhase`](toven_model::ReleasePhase) is
/// satisfied: by Toven's own code or by delegating to an external tool.
///
/// This is the seam-level backing descriptor the engine resolves per phase from
/// config (the config-facing types are
/// [`PhaseBackingKind`](crate::config::PhaseBackingKind) +
/// [`DelegatedTool`](crate::config::DelegatedTool); this is the resolved
/// runtime value, not a directly-deserialized config field). Whichever backing
/// a phase uses, the engine keeps ownership of the flow and its guarantees —
/// mutation-free preview, `--yes` + allowed branch + clean tree for mutation,
/// immutable outputs with forward-fix recovery, and typed reporting. Delegation
/// is **per-phase only**: Toven never hands the whole flow to an external tool,
/// and it invokes any delegated tool argv-first while parsing, guarding, and
/// reporting around it.
///
/// The default is [`Native`](Self::Native); delegation is an explicit,
/// per-phase opt-in. `#[non_exhaustive]` because further backing shapes may be
/// added.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum PhaseBacking {
    /// Toven implements the phase itself — the default.
    #[default]
    Native,
    /// Toven delegates the phase to an external tool, invoked argv-first, while
    /// still owning selection, ordering, readiness, safety, and reporting
    /// around it.
    Delegated {
        /// The external tool that backs the phase (e.g. `goreleaser`). The
        /// *name* of the tool only — never secrets, which flow through the
        /// child-process environment.
        tool: String,
    },
}

impl PhaseBacking {
    /// A native backing.
    #[must_use]
    pub const fn native() -> Self {
        Self::Native
    }

    /// A delegated backing to the named external `tool`.
    #[must_use]
    pub fn delegated(tool: impl Into<String>) -> Self {
        Self::Delegated { tool: tool.into() }
    }

    /// Diagnostic label for the backing kind (`native` or `delegated`), for
    /// reports and error messages.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Delegated { .. } => "delegated",
        }
    }

    /// Whether the phase is backed by Toven's own code.
    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }

    /// The delegated tool name, if this backing delegates.
    #[must_use]
    pub const fn tool(&self) -> Option<&str> {
        match self {
            Self::Native => None,
            Self::Delegated { tool } => Some(tool.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PhaseBacking;

    #[test]
    fn default_is_native() {
        assert_eq!(PhaseBacking::default(), PhaseBacking::Native);
        assert!(PhaseBacking::native().is_native());
        assert_eq!(PhaseBacking::native().as_str(), "native");
        assert_eq!(PhaseBacking::native().tool(), None);
    }

    #[test]
    fn delegated_carries_the_tool_name() {
        let backing = PhaseBacking::delegated("goreleaser");
        assert!(!backing.is_native());
        assert_eq!(backing.as_str(), "delegated");
        assert_eq!(backing.tool(), Some("goreleaser"));
    }
}
