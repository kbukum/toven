//! Project-level generation defaults.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    core::{AppError, AppResult, validate_identifier, validate_name},
    generate::model::{GenerateContext, GenerateDocument, GeneratedProject},
};

const CONFIG_SCHEMA: u16 = 1;

pub(super) fn normalize_root(root: &Path) -> AppResult<PathBuf> {
    let root = fs::canonicalize(root).map_err(|error| {
        AppError::invalid_input(
            "generate.root",
            format!("failed to resolve root '{}': {error}", root.display()),
        )
    })?;
    if !root.is_dir() {
        return Err(AppError::invalid_input(
            "generate.root",
            format!("root '{}' is not a directory", root.display()),
        ));
    }
    Ok(root)
}

pub(super) fn base_document(context: &GenerateContext) -> AppResult<GenerateDocument> {
    validate_identifier("generate.profile", &context.profile_name)?;
    let name = project_name(&context.root);
    validate_name("project.name", &name)?;
    Ok(GenerateDocument {
        project: GeneratedProject {
            schema: CONFIG_SCHEMA,
            name,
            root: PathBuf::from("."),
            base_ref: None,
        },
        profiles: BTreeMap::new(),
        warnings: Vec::new(),
    })
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("project")
        .to_string()
}
