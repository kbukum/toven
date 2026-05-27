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
}

pub(super) struct NormalizedWorkspace {
    pub(super) schema: u16,
    pub(super) name: String,
    pub(super) root: PathBuf,
}

pub(super) fn normalize_workspace_config(
    config: WorkspaceConfig,
    config_path: &Path,
) -> AppResult<NormalizedWorkspace> {
    let schema = validate_schema(config.schema)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let root = normalize_root(config_dir, config.root.as_deref())?;
    let name = normalize_name(config.name, &root)?;

    Ok(NormalizedWorkspace { schema, name, root })
}

pub(super) fn build_workspace(workspace: NormalizedWorkspace, profiles: Vec<Profile>) -> Workspace {
    Workspace {
        schema: workspace.schema,
        name: workspace.name,
        root: workspace.root,
        profiles,
    }
}

fn validate_schema(schema: Option<u16>) -> AppResult<u16> {
    let schema = schema.unwrap_or(SUPPORTED_SCHEMA);
    if schema != SUPPORTED_SCHEMA {
        return Err(AppError::invalid_input(
            "workspace.schema",
            format!("unsupported schema {schema}; supported schema is {SUPPORTED_SCHEMA}"),
        ));
    }
    Ok(schema)
}

fn normalize_root(config_dir: &Path, root: Option<&Path>) -> AppResult<PathBuf> {
    let root = root.unwrap_or_else(|| Path::new("."));
    let root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        config_dir.join(root)
    };
    rskit_fs::canonicalize(&root).map_err(|error| {
        AppError::invalid_input(
            "workspace.root",
            format!("failed to resolve workspace root '{}'", root.display()),
        )
        .with_cause(error)
    })
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
