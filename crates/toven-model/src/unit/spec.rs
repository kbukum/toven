//! [`Unit`] — a single Toven action, identified by name and satisfied by a
//! [`Backing`](super::Backing).

use serde::{Deserialize, Serialize};

use super::{Backing, Composite};

/// A single Toven action on the shared PLAN → APPLY spine.
///
/// Every action a user can invoke — a task, a native capability (bump, tag,
/// publish, …), a delegated tool, or a composite chain — is a `Unit` with a
/// name identity and a [`Backing`](super::Backing) that says how it is
/// satisfied. Units are otherwise uniform: discovered, planned mutation-free,
/// gated on apply, and reported through the same typed surface, differing only
/// in their backing.
///
/// Vocabulary only: this type *names* the action and its backing; the execution
/// spine that consumes it uniformly is built in later steps. `#[non_exhaustive]`
/// because a unit will grow further attributes (hooks, fan-out) as the spine
/// generalizes.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Unit {
    /// The unit's identity: the name a user invokes (`toven <name>`).
    pub name: String,
    /// How this unit is satisfied.
    pub backing: Backing,
}

impl Unit {
    /// A unit with the given `name` identity and `backing`.
    #[must_use]
    pub fn new(name: impl Into<String>, backing: Backing) -> Self {
        Self {
            name: name.into(),
            backing,
        }
    }

    /// An argv (task) unit named `name`.
    #[must_use]
    pub fn argv(name: impl Into<String>) -> Self {
        Self::new(name, Backing::Argv)
    }

    /// A native-capability unit named `name`.
    #[must_use]
    pub fn native(name: impl Into<String>) -> Self {
        Self::new(name, Backing::Native)
    }

    /// A composite unit named `name` over the ordered `units`.
    #[must_use]
    pub fn composite(name: impl Into<String>, units: impl Into<Composite>) -> Self {
        Self::new(name, Backing::composite(units))
    }

    /// The unit's name identity.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How this unit is satisfied.
    #[must_use]
    pub const fn backing(&self) -> &Backing {
        &self.backing
    }
}

#[cfg(test)]
mod tests {
    use super::{Backing, Unit};

    #[test]
    fn argv_unit_carries_an_argv_backing() {
        let unit = Unit::argv("test");
        assert_eq!(unit.name(), "test");
        assert_eq!(unit.backing(), &Backing::Argv);
    }

    #[test]
    fn native_unit_carries_a_native_backing() {
        let unit = Unit::native("bump");
        assert_eq!(unit.name(), "bump");
        assert!(unit.backing().is_native());
    }

    #[test]
    fn composite_unit_nests_its_members() {
        let unit = Unit::composite("release", vec![Unit::native("bump"), Unit::native("tag")]);
        assert_eq!(unit.name(), "release");
        assert_eq!(unit.backing().as_str(), "composite");
    }

    #[test]
    fn round_trips_through_json() {
        let unit = Unit::composite(
            "release",
            vec![
                Unit::native("bump"),
                Unit::argv("test"),
                Unit::native("tag"),
            ],
        );
        let json = serde_json::to_string(&unit).expect("serialize");
        let back: Unit = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(unit, back);
    }
}
