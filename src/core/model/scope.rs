//! Scope-level model.

use std::{fmt, str::FromStr};

use crate::core::{AppError, AppResult, validate_identifier};

/// Unique scope identifier within a project.
#[derive(
    Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct ScopeId(String);

impl ScopeId {
    /// Create a scope identifier from a validated string.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        Self::parse(value)
    }

    /// Return the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse and validate a scope identifier.
    pub fn parse(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        validate_identifier("scope.id", &value)?;
        Ok(Self(value))
    }
}

impl TryFrom<String> for ScopeId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ScopeId> for String {
    fn from(value: ScopeId) -> Self {
        value.0
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ScopeId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Stable adapter identifier.
#[derive(
    Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct AdapterId(String);

impl AdapterId {
    /// Create an adapter identifier from a validated string.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        Self::parse(value)
    }

    /// Return the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse and validate an adapter identifier.
    pub fn parse(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        validate_identifier("adapter.id", &value)?;
        Ok(Self(value))
    }
}

impl TryFrom<String> for AdapterId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<AdapterId> for String {
    fn from(value: AdapterId) -> Self {
        value.0
    }
}

impl fmt::Display for AdapterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for AdapterId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{AdapterId, ScopeId};

    #[test]
    fn scope_id_exposes_value() {
        let id = ScopeId::new("core").expect("scope id parses");

        assert_eq!(id.as_str(), "core");
    }

    #[test]
    fn adapter_id_exposes_value() {
        let id = AdapterId::new("rust").expect("adapter id parses");

        assert_eq!(id.as_str(), "rust");
    }
}
