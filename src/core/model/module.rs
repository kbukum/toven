//! Module-level model.

use std::{fmt, path::PathBuf, str::FromStr};

use crate::core::{AdapterId, AppError, AppResult, ScopeId, validate_identifier};

/// Scope-qualified module identifier.
pub type ScopedModuleKey = (String, ModuleId);

/// Explicit project-level dependency edge between scope-qualified modules.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct DependencyOverlay {
    /// Module that depends on `to`.
    pub from: ScopedModuleKey,
    /// Module required by `from`.
    pub to: ScopedModuleKey,
}

/// Unique module identifier within a workspace.
#[derive(
    Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct ModuleId(String);

impl ModuleId {
    /// Create a module identifier from a validated string.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        Self::parse(value)
    }

    /// Return the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse and validate a module identifier.
    pub fn parse(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        validate_identifier("module.name", &value)?;
        Ok(Self(value))
    }
}

impl TryFrom<String> for ModuleId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ModuleId> for String {
    fn from(value: ModuleId) -> Self {
        value.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ModuleId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// A discovered module independent of language-specific manifests.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Module {
    /// Scope that owns the module.
    pub scope_id: ScopeId,
    /// Adapter that discovered the module.
    pub adapter_id: AdapterId,
    /// Unique module identifier.
    pub name: ModuleId,
    /// Optional package name used by command templates.
    pub package: Option<String>,
    /// Module root relative to the workspace root.
    pub root: PathBuf,
    /// Manifest/discovery-unit path relative to the workspace root.
    pub manifest: Option<PathBuf>,
    /// Module identifiers this module depends on.
    pub dependencies: Vec<ModuleId>,
    /// Glob-like source patterns relative to the workspace root.
    pub source_patterns: Vec<String>,
}

/// Return a stable scope-qualified key for a module.
#[must_use]
pub fn scoped_module_key(module: &Module) -> ScopedModuleKey {
    (module.scope_id.to_string(), module.name.clone())
}

/// Render a scope-qualified module key for human-facing output.
#[must_use]
pub fn scoped_module_display(key: &ScopedModuleKey) -> String {
    format!("{}/{}", key.0, key.1)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ModuleId;

    #[test]
    fn module_id_exposes_value() {
        let id = ModuleId::new("core").expect("module id parses");

        assert_eq!(id.as_str(), "core");
    }

    #[test]
    fn module_id_converts_into_string() {
        let id = ModuleId::new("core").expect("module id parses");

        assert_eq!(String::from(id), "core");
    }

    #[test]
    fn module_id_parse_rejects_empty_values() {
        let error = ModuleId::parse(" ").expect_err("empty value should fail");

        assert!(error.message.contains("is required"));
    }

    #[test]
    fn module_id_parse_rejects_surrounding_whitespace() {
        let error = ModuleId::parse(" api ").expect_err("surrounding whitespace should fail");

        assert!(
            error
                .message
                .contains("cannot contain leading or trailing whitespace")
        );
    }

    #[test]
    fn module_id_parse_rejects_path_unsafe_values() {
        for value in ["../api", "api/core", "api\\core", "api:core", ".", ".."] {
            let error = ModuleId::parse(value).expect_err("path-unsafe value should fail");

            assert!(
                error
                    .message
                    .contains("cannot contain path separators or traversal markers")
            );
        }
    }

    #[test]
    fn module_id_parse_rejects_control_characters() {
        let error = ModuleId::parse("api\u{202e}core").expect_err("control value should fail");

        assert!(error.message.contains("control characters"));
    }

    #[test]
    fn module_id_implements_from_str() {
        let id = ModuleId::from_str("api").expect("module id parses");

        assert_eq!(id.to_string(), "api");
    }

    #[test]
    fn module_id_try_from_rejects_empty_values() {
        let error = ModuleId::try_from(String::from(" ")).expect_err("empty value should fail");

        assert!(error.message.contains("is required"));
    }

    #[test]
    fn module_id_try_from_rejects_surrounding_whitespace() {
        let error = ModuleId::try_from(String::from(" api "))
            .expect_err("surrounding whitespace should fail");

        assert!(
            error
                .message
                .contains("cannot contain leading or trailing whitespace")
        );
    }

    #[test]
    fn module_id_deserialization_rejects_empty_values() {
        use serde::Deserialize as _;

        let deserializer =
            serde::de::value::StringDeserializer::<serde::de::value::Error>::new(" ".to_string());
        let error = ModuleId::deserialize(deserializer).expect_err("empty value should fail");

        assert!(error.to_string().contains("is required"));
    }

    #[test]
    fn module_id_deserialization_rejects_surrounding_whitespace() {
        use serde::Deserialize as _;

        let deserializer = serde::de::value::StringDeserializer::<serde::de::value::Error>::new(
            " api ".to_string(),
        );
        let error =
            ModuleId::deserialize(deserializer).expect_err("surrounding whitespace should fail");

        assert!(
            error
                .to_string()
                .contains("cannot contain leading or trailing whitespace")
        );
    }
}
