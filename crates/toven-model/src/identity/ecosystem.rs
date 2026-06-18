//! Ecosystem identifier newtype.

use std::{fmt, str::FromStr};

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::ecosystems;

/// Identifier for a language/package ecosystem (e.g. `rust`, `go`).
///
/// The value is validated to be a non-empty, path-safe identifier with no `:`
/// separators, so it composes cleanly into a [`ModuleRef`](crate::ModuleRef)
/// `ecosystem:name` rendering. Canonicity (whether the id is a known ecosystem)
/// is a *separate* concern resolved against the [`ecosystems`] registry; an
/// `EcosystemId` may be syntactically valid yet non-canonical (a typo).
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct EcosystemId(String);

impl EcosystemId {
    /// Validate and construct an ecosystem identifier.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        rskit_validation::input::validate_path_safe_identifier("ecosystem", &value)?;
        Ok(Self(value))
    }

    /// Borrow the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this id appears in the canonical ecosystem registry.
    #[must_use]
    pub fn is_canonical(&self) -> bool {
        ecosystems::is_canonical(&self.0)
    }

    /// Human-readable label for this id when it is canonical.
    #[must_use]
    pub fn label(&self) -> Option<&'static str> {
        ecosystems::label(&self.0)
    }
}

impl fmt::Display for EcosystemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for EcosystemId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for EcosystemId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<EcosystemId> for String {
    fn from(value: EcosystemId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::EcosystemId;

    #[test]
    fn rejects_separator_and_empty() {
        assert!(EcosystemId::new("rust:errors").is_err());
        assert!(EcosystemId::new("  ").is_err());
        assert!(EcosystemId::new("rust").is_ok());
    }

    #[test]
    fn reports_canonicity() {
        assert!(EcosystemId::new("rust").unwrap().is_canonical());
        assert_eq!(EcosystemId::new("rust").unwrap().label(), Some("Rust"));
        assert!(!EcosystemId::new("rsut").unwrap().is_canonical());
        assert_eq!(EcosystemId::new("rsut").unwrap().label(), None);
    }
}
