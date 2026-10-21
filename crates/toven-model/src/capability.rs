//! Module capabilities: opaque facts an adapter reports about what a module
//! provides, matched against a task's required capabilities.

use std::fmt;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Upper bound on a capability's length, so a hostile manifest cannot inflate
/// plans and events with unbounded labels.
const MAX_LEN: usize = 128;

/// One capability a module provides (`bin`, `script:test`).
///
/// The model treats the value as opaque: adapters decide what they report and
/// tasks decide what they require. A capability is 1 to 128 visible ASCII
/// characters with no whitespace, so it renders safely in plans, events, and
/// config.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Capability(String);

impl Capability {
    /// Validate and construct a capability.
    ///
    /// # Errors
    /// Returns [`AppError::invalid_input`] when the value is empty, longer than
    /// 128 characters, or contains anything other than visible ASCII.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_LEN {
            return Err(AppError::invalid_input(
                "capability",
                format!("'{value}' must be 1 to {MAX_LEN} characters"),
            ));
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(AppError::invalid_input(
                "capability",
                format!("'{value}' must contain only visible ASCII characters"),
            ));
        }
        Ok(Self(value))
    }

    /// Borrow the capability as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for Capability {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Capability> for String {
    fn from(value: Capability) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::Capability;

    #[test]
    fn accepts_namespaced_values() {
        assert_eq!(
            Capability::new("script:test:unit").unwrap().as_str(),
            "script:test:unit"
        );
        assert_eq!(Capability::new("bin").unwrap().to_string(), "bin");
    }

    #[test]
    fn rejects_empty_whitespace_and_control_characters() {
        assert!(Capability::new("").is_err());
        assert!(Capability::new("script: test").is_err());
        assert!(Capability::new("bin\n").is_err());
        assert!(Capability::new("ünicode").is_err());
    }

    #[test]
    fn rejects_overlong_values() {
        assert!(Capability::new("a".repeat(128)).is_ok());
        assert!(Capability::new("a".repeat(129)).is_err());
    }

    #[test]
    fn deserialization_validates() {
        assert!(serde_json::from_str::<Capability>("\"bin\"").is_ok());
        assert!(serde_json::from_str::<Capability>("\"\"").is_err());
    }
}
