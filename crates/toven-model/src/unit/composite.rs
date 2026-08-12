//! [`Composite`] — an ordered chain of [`Unit`](super::Unit)s run as one.

use serde::{Deserialize, Serialize};

use super::Unit;

/// An ordered chain of [`Unit`](super::Unit)s executed as a single composite
/// unit, in declaration order (e.g. `release = bump → tag → publish`).
///
/// Vocabulary only: it *names* the ordered members of a composite so config,
/// planning, and reporting can refer to them. This step lands the type; the
/// composite's execution semantics (ordered barriers, per-ecosystem fan-out)
/// arrive with the composite-execution step. `#[non_exhaustive]` because a
/// composite may later carry ordering/barrier policy beyond its members.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Composite {
    /// The ordered member units of the chain.
    pub units: Vec<Unit>,
}

impl Composite {
    /// A composite over the given ordered `units`.
    #[must_use]
    pub const fn new(units: Vec<Unit>) -> Self {
        Self { units }
    }

    /// The ordered member units of the chain.
    #[must_use]
    pub const fn units(&self) -> &[Unit] {
        self.units.as_slice()
    }

    /// Whether the chain has no members.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}

impl From<Vec<Unit>> for Composite {
    fn from(units: Vec<Unit>) -> Self {
        Self::new(units)
    }
}

#[cfg(test)]
mod tests {
    use super::{Composite, Unit};

    #[test]
    fn empty_composite_has_no_members() {
        let composite = Composite::default();
        assert!(composite.is_empty());
        assert!(composite.units().is_empty());
    }

    #[test]
    fn preserves_member_order() {
        let composite = Composite::new(vec![
            Unit::native("bump"),
            Unit::native("tag"),
            Unit::native("publish"),
        ]);
        assert!(!composite.is_empty());
        let names: Vec<&str> = composite.units().iter().map(Unit::name).collect();
        assert_eq!(names, ["bump", "tag", "publish"]);
    }

    #[test]
    fn from_vec_wraps_the_units() {
        let composite = Composite::from(vec![Unit::argv("test")]);
        assert_eq!(composite.units().len(), 1);
        assert_eq!(composite.units()[0].name(), "test");
    }

    #[test]
    fn round_trips_through_json() {
        let composite = Composite::new(vec![Unit::native("bump"), Unit::argv("test")]);
        let json = serde_json::to_string(&composite).expect("serialize");
        let back: Composite = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(composite, back);
    }
}
