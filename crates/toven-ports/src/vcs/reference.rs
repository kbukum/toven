//! Git object id and tag reference value types.

use serde::{Deserialize, Serialize};

/// A git object id (commit/tag/tree hash), kept as an opaque string.
///
/// Toven never parses it; it flows from a `rev_parse`/`merge_base` resolution
/// back into a [`BaselineSpec`](super::BaselineSpec) or diff call.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct Oid(String);

impl Oid {
    /// Wrap a resolved object id string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A resolved tag reference (release baselines / `<module>@<version>` tags).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct TagRef {
    /// Tag name (e.g. `rust-errors@1.2.0`).
    pub name: String,
    /// Object id the tag points at.
    pub target: Oid,
}

impl TagRef {
    /// Construct a tag reference.
    #[must_use]
    pub fn new(name: impl Into<String>, target: Oid) -> Self {
        Self {
            name: name.into(),
            target,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Oid, TagRef};

    #[test]
    fn oid_borrows_inner_string() {
        let oid = Oid::new("deadbeef");
        assert_eq!(oid.as_str(), "deadbeef");
    }

    #[test]
    fn tag_ref_carries_name_and_target() {
        let tag = TagRef::new("rust:errors@1.2.0", Oid::new("cafe"));
        assert_eq!(tag.name, "rust:errors@1.2.0");
        assert_eq!(tag.target.as_str(), "cafe");
    }

    #[test]
    fn round_trips_through_toml() {
        let tag = TagRef::new("rust:errors@1.2.0", Oid::new("cafe"));
        let json = toml::to_string(&tag).expect("serialize");
        let back: TagRef = toml::from_str(&json).expect("deserialize");
        assert_eq!(tag, back);
    }
}
