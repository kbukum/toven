//! Cross-repo member identifier.

use std::fmt;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Identifier of a repository member in a cross-repo federation.
///
/// `None` on a module means the single-repo case; `Some` scopes the module to
/// one `[[members]]` entry in a cross-repo umbrella. Member ids also appear in
/// text refs as the `member/` qualifier, so they must be path-safe single
/// segments and cannot contain `/`.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct MemberId(String);

impl MemberId {
    /// Validate and construct the identifier.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        rskit_validation::input::validate_path_safe_identifier("member.id", &value)?;
        Ok(Self(value))
    }

    /// Borrow the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemberId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for MemberId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MemberId> for String {
    fn from(value: MemberId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::MemberId;

    #[test]
    fn rejects_blank_values() {
        assert!(MemberId::new("").is_err());
    }

    #[test]
    fn exposes_value() {
        assert_eq!(MemberId::new("repo-a").unwrap().to_string(), "repo-a");
    }

    #[test]
    fn rejects_ref_separator() {
        assert!(MemberId::new("team/repo").is_err());
    }
}
