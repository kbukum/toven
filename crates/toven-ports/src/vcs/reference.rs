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
