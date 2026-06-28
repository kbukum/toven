//! Workspace identifier.

use std::fmt;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Identifier of a discovery unit (one Cargo workspace, one `go.work`, ...).
///
/// Metadata on [`Module`](crate::Module); links a module to the
/// [`Workspace`](crate::Workspace) that carries its toolchain and resource
/// grouping.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// Validate and construct the identifier (must be non-empty).
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        rskit_validation::input::validate_required_trimmed("workspace.id", &value)?;
        Ok(Self(value))
    }

    /// Borrow the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for WorkspaceId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<WorkspaceId> for String {
    fn from(value: WorkspaceId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceId;

    #[test]
    fn rejects_blank_values() {
        assert!(WorkspaceId::new("  ").is_err());
    }

    #[test]
    fn exposes_value() {
        assert_eq!(WorkspaceId::new("core").unwrap().as_str(), "core");
    }
}
