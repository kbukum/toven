//! Module reference: the identity used everywhere (`ecosystem:name`).

use std::{fmt, str::FromStr};

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use super::EcosystemId;

/// Stable identity of a module, unique across all ecosystems.
///
/// Renders as `ecosystem:name` (e.g. `rust:errors`). The `name` is unique
/// *within* its ecosystem; the `ecosystem` prefix guarantees global uniqueness
/// so federation can union module sets without collision. The owning workspace
/// is metadata on [`Module`](crate::Module), never part of identity.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
pub struct ModuleRef {
    /// Ecosystem that owns the module.
    pub ecosystem: EcosystemId,
    /// Module name, unique within the ecosystem (must not contain `:`).
    pub name: String,
}

impl ModuleRef {
    /// Validate and construct a module reference.
    pub fn new(ecosystem: EcosystemId, name: impl Into<String>) -> AppResult<Self> {
        let name = name.into();
        rskit_validation::input::validate_path_safe_identifier("module.name", &name)?;
        Ok(Self { ecosystem, name })
    }

    /// Parse the canonical `ecosystem:name` rendering.
    pub fn parse(value: &str) -> AppResult<Self> {
        let (ecosystem, name) = value.split_once(':').ok_or_else(|| {
            AppError::invalid_input(
                "module",
                format!("expected 'ecosystem:name', got '{value}'"),
            )
        })?;
        Self::new(EcosystemId::new(ecosystem)?, name)
    }
}

impl fmt::Display for ModuleRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.ecosystem, self.name)
    }
}

impl FromStr for ModuleRef {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{EcosystemId, ModuleRef};

    fn module_ref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    #[test]
    fn display_and_parse_round_trip() {
        let reference = module_ref("rust", "errors");
        assert_eq!(reference.to_string(), "rust:errors");
        assert_eq!(ModuleRef::parse("rust:errors").unwrap(), reference);
        assert_eq!("rust:errors".parse::<ModuleRef>().unwrap(), reference);
    }

    #[test]
    fn parse_rejects_missing_separator() {
        assert!(ModuleRef::parse("errors").is_err());
    }

    #[test]
    fn rejects_name_with_separator() {
        assert!(ModuleRef::new(EcosystemId::new("rust").unwrap(), "a:b").is_err());
    }
}
