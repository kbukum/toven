//! [`Backing`] — how a single [`Unit`](super::Unit) is satisfied.

use serde::{Deserialize, Serialize};

use super::Composite;

/// How a single [`Unit`](super::Unit) is satisfied: the one axis on which
/// otherwise-uniform units differ.
///
/// Every Toven action flows through the same PLAN → APPLY spine and differs
/// only in its backing:
/// - [`Argv`](Self::Argv) — a CLI task (`test`, `build`, `lint`) run by the
///   process engine as an argument vector.
/// - [`Native`](Self::Native) — one of Toven's own ecosystem-adapter
///   capabilities (bump, tag, publish, coverage, …), implemented in Rust with
///   no external tool. The default.
/// - [`Delegated`](Self::Delegated) — a native capability handed to an external
///   tool invoked argv-first, while Toven keeps ownership of selection,
///   ordering, readiness, safety, and reporting around it.
/// - [`Composite`](Self::Composite) — an ordered chain of units (e.g.
///   `release = bump → tag → publish`).
///
/// This is the one backing axis for the whole system — a task, a native
/// capability, a delegated tool, or a composite chain all differ only here. The
/// default is [`Native`](Self::Native); every other backing is an explicit
/// choice. `#[non_exhaustive]` because further backing shapes may be added.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Backing {
    /// A CLI task run as an argument vector by the process engine.
    Argv,
    /// A capability Toven implements itself — the default.
    #[default]
    Native,
    /// A capability delegated to an external tool, invoked argv-first, while
    /// Toven still owns selection, ordering, readiness, safety, and reporting
    /// around it.
    Delegated {
        /// The external tool that backs the capability (e.g. `goreleaser`). The
        /// *name* of the tool only — never secrets, which flow through the
        /// child-process environment.
        tool: String,
    },
    /// An ordered chain of units run as one composite (e.g.
    /// `release = bump → tag → publish`).
    Composite(Composite),
}

impl Backing {
    /// An argv (task) backing.
    #[must_use]
    pub const fn argv() -> Self {
        Self::Argv
    }

    /// A native backing — the default.
    #[must_use]
    pub const fn native() -> Self {
        Self::Native
    }

    /// A delegated backing to the named external `tool`.
    #[must_use]
    pub fn delegated(tool: impl Into<String>) -> Self {
        Self::Delegated { tool: tool.into() }
    }

    /// A composite backing over an ordered chain of `units`.
    #[must_use]
    pub fn composite(units: impl Into<Composite>) -> Self {
        Self::Composite(units.into())
    }

    /// Diagnostic label for the backing kind (`argv`, `native`, `delegated`, or
    /// `composite`), for reports and error messages.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Argv => "argv",
            Self::Native => "native",
            Self::Delegated { .. } => "delegated",
            Self::Composite(_) => "composite",
        }
    }

    /// Whether the unit is backed by Toven's own code (the native default).
    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }

    /// The delegated tool name, if this backing delegates.
    #[must_use]
    pub const fn tool(&self) -> Option<&str> {
        match self {
            Self::Delegated { tool } => Some(tool.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Backing;

    #[test]
    fn default_is_native() {
        assert_eq!(Backing::default(), Backing::Native);
        assert!(Backing::native().is_native());
        assert_eq!(Backing::native().as_str(), "native");
        assert_eq!(Backing::native().tool(), None);
    }

    #[test]
    fn argv_backs_a_task() {
        let backing = Backing::argv();
        assert_eq!(backing, Backing::Argv);
        assert!(!backing.is_native());
        assert_eq!(backing.as_str(), "argv");
        assert_eq!(backing.tool(), None);
    }

    #[test]
    fn delegated_carries_the_tool_name() {
        let backing = Backing::delegated("goreleaser");
        assert!(!backing.is_native());
        assert_eq!(backing.as_str(), "delegated");
        assert_eq!(backing.tool(), Some("goreleaser"));
    }

    #[test]
    fn composite_reports_its_kind() {
        let backing = Backing::composite(Vec::new());
        assert!(!backing.is_native());
        assert_eq!(backing.as_str(), "composite");
        assert_eq!(backing.tool(), None);
    }

    #[test]
    fn round_trips_through_json() {
        for backing in [
            Backing::Argv,
            Backing::Native,
            Backing::delegated("goreleaser"),
        ] {
            let json = serde_json::to_string(&backing).expect("serialize");
            let back: Backing = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(backing, back);
        }
    }
}
