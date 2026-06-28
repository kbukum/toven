//! Opaque identifier newtypes for workspaces and cross-repo members.

use std::fmt;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident, $field:literal) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize,
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validate and construct the identifier (must be non-empty).
            pub fn new(value: impl Into<String>) -> AppResult<Self> {
                let value = value.into();
                rskit_validation::input::validate_required_trimmed($field, &value)?;
                Ok(Self(value))
            }

            /// Borrow the identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = AppError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

string_id!(
    /// Identifier of a discovery unit (one Cargo workspace, one `go.work`, …).
    ///
    /// Metadata on [`Module`](crate::Module); links a module to the
    /// [`Workspace`](crate::Workspace) that carries its toolchain and resource
    /// grouping.
    WorkspaceId,
    "workspace.id"
);

string_id!(
    /// Identifier of a repository member in a cross-repo federation.
    ///
    /// `None` on a module means the single-repo case; `Some` scopes the module
    /// to one `[[members]]` entry in a cross-repo umbrella.
    MemberId,
    "member.id"
);

#[cfg(test)]
mod tests {
    use super::{MemberId, WorkspaceId};

    #[test]
    fn rejects_blank_values() {
        assert!(WorkspaceId::new("  ").is_err());
        assert!(MemberId::new("").is_err());
    }

    #[test]
    fn exposes_value() {
        assert_eq!(WorkspaceId::new("core").unwrap().as_str(), "core");
        assert_eq!(MemberId::new("repo-a").unwrap().to_string(), "repo-a");
    }
}
