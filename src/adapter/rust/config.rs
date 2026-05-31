//! Rust adapter profile options.

use std::path::{Component, Path, PathBuf};

use crate::core::{AdapterOptions, AppError, AppResult};

const DEFAULT_MANIFEST: &str = "Cargo.toml";

/// Rust adapter options for one profile.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RustProfileOptions {
    /// Cargo manifest paths relative to the project root.
    #[serde(default = "default_manifests")]
    pub manifests: Vec<PathBuf>,
}

impl RustProfileOptions {
    /// Build Rust options from discovery manifests.
    pub fn from_manifests(manifests: Vec<PathBuf>) -> AppResult<Self> {
        let options = Self {
            manifests: if manifests.is_empty() {
                default_manifests()
            } else {
                manifests
            },
        };
        validate_manifests("profiles.<profile>.manifests", &options.manifests)?;
        Ok(options)
    }

    /// Decode Rust options carried by the discovery protocol.
    pub fn from_adapter_options(options: &AdapterOptions) -> AppResult<Self> {
        let value = serde_json::Value::Object(options.clone().into_iter().collect());
        let options: Self = serde_json::from_value(value).map_err(|error| {
            AppError::invalid_input(
                "profiles.<profile>.manifests",
                format!("invalid rust options: {error}"),
            )
        })?;
        validate_manifests("profiles.<profile>.manifests", &options.manifests)?;
        Ok(options)
    }

    /// Encode Rust options for the discovery protocol.
    pub fn to_adapter_options(&self) -> AppResult<AdapterOptions> {
        let serde_json::Value::Object(map) =
            serde_json::to_value(self).map_err(AppError::internal)?
        else {
            return Err(AppError::internal(std::io::Error::other(
                "rust options did not serialize as object",
            )));
        };
        Ok(map.into_iter().collect())
    }
}

/// Default Rust manifest path.
#[must_use]
pub fn default_manifest() -> PathBuf {
    PathBuf::from(DEFAULT_MANIFEST)
}

fn default_manifests() -> Vec<PathBuf> {
    vec![default_manifest()]
}

fn validate_manifests(field: &str, manifests: &[PathBuf]) -> AppResult<()> {
    if manifests.is_empty() {
        return Err(AppError::invalid_input(
            field,
            "at least one manifest is required",
        ));
    }
    for manifest in manifests {
        validate_relative_path(field, manifest)?;
    }
    Ok(())
}

fn validate_relative_path(field: &str, path: &Path) -> AppResult<()> {
    if path.is_absolute() {
        return Err(AppError::invalid_input(field, "path must be relative"));
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::invalid_input(
                    field,
                    "path cannot contain traversal or root components",
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}
