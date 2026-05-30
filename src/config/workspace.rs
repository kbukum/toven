//! Workspace-level config normalization.

use std::path::{Path, PathBuf};

use crate::core::{AppError, AppResult, Profile, Workspace, validate_name};

const SUPPORTED_SCHEMA: u16 = 1;

/// `[workspace]` table from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Config schema version.
    pub schema: Option<u16>,
    /// Human-readable workspace name.
    pub name: Option<String>,
    /// Workspace root, relative to the config file unless absolute.
    pub root: Option<PathBuf>,
    /// Default git baseline reference for affected detection.
    pub base_ref: Option<String>,
}

pub(super) struct NormalizedWorkspace {
    pub(super) schema: u16,
    pub(super) name: String,
    pub(super) root: PathBuf,
    pub(super) base_ref: Option<String>,
}

pub(super) fn normalize_workspace_config(
    config: WorkspaceConfig,
    config_path: &Path,
) -> AppResult<NormalizedWorkspace> {
    let schema = validate_schema(config.schema)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let root = normalize_root(config_dir, config.root.as_deref())?;
    let name = normalize_name(config.name, &root)?;

    let base_ref = normalize_base_ref(config.base_ref)?;

    Ok(NormalizedWorkspace {
        schema,
        name,
        root,
        base_ref,
    })
}

pub(super) fn build_workspace(workspace: NormalizedWorkspace, profiles: Vec<Profile>) -> Workspace {
    Workspace {
        schema: workspace.schema,
        name: workspace.name,
        root: workspace.root,
        base_ref: workspace.base_ref,
        profiles,
    }
}

fn validate_schema(schema: Option<u16>) -> AppResult<u16> {
    rskit_config::supported_schema("workspace.schema", schema, SUPPORTED_SCHEMA)
}

fn normalize_root(config_dir: &Path, root: Option<&Path>) -> AppResult<PathBuf> {
    rskit_config::canonicalize_root_relative_to("workspace.root", config_dir, root)
}

fn normalize_name(name: Option<String>, root: &Path) -> AppResult<String> {
    match name {
        Some(name) => {
            validate_name("workspace.name", &name)?;
            Ok(name)
        }
        None => Ok(root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("workspace")
            .to_string()),
    }
}

fn normalize_base_ref(base_ref: Option<String>) -> AppResult<Option<String>> {
    base_ref
        .map(|base_ref| {
            let trimmed = base_ref.trim();
            if trimmed.is_empty() {
                return Err(AppError::invalid_input(
                    "workspace.base_ref",
                    "base_ref cannot be empty",
                ));
            }
            if trimmed != base_ref {
                return Err(AppError::invalid_input(
                    "workspace.base_ref",
                    "base_ref cannot contain leading or trailing whitespace",
                ));
            }
            Ok(base_ref)
        })
        .transpose()
}
