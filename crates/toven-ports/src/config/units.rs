//! User-declared composite-unit vocabulary: an ordered chain of member unit
//! references run as one composite (`[units.<name>]`).
//!
//! This is a **verb-agnostic** concern. A project declares a named chain that
//! composes existing units (native capabilities such as `bump`/`tag`/`publish`,
//! or another declared composite) into one action, letting a user express a
//! release-like flow without changing argv. The engine resolves and executes
//! the chain later; this type carries only the ordered member references and
//! their field-level validation.

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// A user-declared composite unit: the ordered member references it chains.
///
/// Declared as `[units.<name>]` with a `chain` list in declaration order (e.g.
/// `chain = ["bump", "tag", "publish"]`). Each member names another unit — a
/// built-in native capability or another declared composite — run in order as
/// one composite unit. The chain is a plain ordered list of names, not a set:
/// a repeated member is kept and runs once per occurrence.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeUnitConfig {
    /// The ordered member unit references of the chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<String>,
}

impl CompositeUnitConfig {
    /// A composite over the given ordered member references.
    #[must_use]
    pub fn new(chain: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            chain: chain.into_iter().map(Into::into).collect(),
        }
    }

    /// The ordered member unit references of the chain.
    #[must_use]
    pub const fn chain(&self) -> &[String] {
        self.chain.as_slice()
    }

    /// Validate the composite's own fields: a non-empty chain of non-blank
    /// member references.
    ///
    /// Structural only — whether each member resolves to a known unit and
    /// whether the chain is acyclic is validated by the engine against the full
    /// set of declared and built-in units.
    ///
    /// # Errors
    /// Rejects an empty chain or a blank member reference.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if self.chain.is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.chain"),
                "a composite unit must chain at least one member unit",
            ));
        }
        for (index, member) in self.chain.iter().enumerate() {
            if member.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.chain[{index}]"),
                    "member unit reference must not be blank",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CompositeUnitConfig;

    #[test]
    fn new_collects_member_references_in_order() {
        let composite = CompositeUnitConfig::new(["bump", "tag", "publish"]);
        assert_eq!(composite.chain(), ["bump", "tag", "publish"]);
    }

    #[test]
    fn deserializes_from_a_chain_list() {
        let composite: CompositeUnitConfig =
            toml::from_str("chain = [\"bump\", \"tag\", \"publish\"]").expect("parse");
        assert_eq!(composite.chain(), ["bump", "tag", "publish"]);
    }

    #[test]
    fn rejects_an_unknown_field() {
        let error = toml::from_str::<CompositeUnitConfig>("steps = [\"bump\"]").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn round_trips_through_json() {
        let composite = CompositeUnitConfig::new(["bump", "tag"]);
        let json = serde_json::to_string(&composite).expect("serialize");
        let back: CompositeUnitConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(composite, back);
    }

    #[test]
    fn validate_accepts_a_non_empty_chain() {
        CompositeUnitConfig::new(["bump", "tag"])
            .validate("units.release")
            .expect("valid");
    }

    #[test]
    fn validate_rejects_an_empty_chain() {
        let error = CompositeUnitConfig::default()
            .validate("units.release")
            .unwrap_err();
        assert!(error.to_string().contains("at least one member"));
    }

    #[test]
    fn validate_rejects_a_blank_member() {
        let error = CompositeUnitConfig::new(["bump", "  "])
            .validate("units.release")
            .unwrap_err();
        assert!(error.to_string().contains("must not be blank"));
    }
}
