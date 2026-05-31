#![allow(clippy::redundant_pub_crate)]

use std::path::{Component, Path};

use super::{AppError, AppResult, Template};

pub(crate) fn validate_name(field: impl AsRef<str>, value: &str) -> AppResult<()> {
    rskit_validation::input::validate_required_trimmed(field.as_ref(), value)
}

pub(crate) fn validate_identifier(field: impl AsRef<str>, value: &str) -> AppResult<()> {
    rskit_validation::input::validate_path_safe_identifier(field.as_ref(), value)
}

pub(crate) fn validate_command_template(
    field: impl AsRef<str>,
    values: &[String],
) -> AppResult<()> {
    let field = field.as_ref();
    if values.is_empty() {
        return Err(AppError::invalid_input(
            field,
            "at least one argv item is required",
        ));
    }
    validate_templates(field, values)
}

pub(crate) fn validate_templates(field: impl AsRef<str>, values: &[String]) -> AppResult<()> {
    let field = field.as_ref();
    for value in values {
        validate_template(field, value)?;
    }
    Ok(())
}

pub(crate) fn validate_template(field: impl AsRef<str>, value: &str) -> AppResult<()> {
    let field = field.as_ref();
    Template::parse(value).map_err(|error| {
        AppError::invalid_input(
            field,
            format!("invalid template '{value}': {}", error.message),
        )
    })?;
    Ok(())
}

pub(crate) fn validate_shared_inputs(field: impl AsRef<str>, values: &[String]) -> AppResult<()> {
    let field = field.as_ref();
    for value in values {
        validate_shared_input(field, value)?;
    }
    Ok(())
}

fn validate_shared_input(field: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(AppError::invalid_input(
            field,
            "shared input paths cannot be empty",
        ));
    }
    if value.trim() != value {
        return Err(AppError::invalid_input(
            field,
            "shared input paths cannot contain leading or trailing whitespace",
        ));
    }
    if value.contains('{') || value.contains('}') {
        return Err(AppError::invalid_input(
            field,
            "shared input paths are plain workspace-relative paths and do not support templates",
        ));
    }
    if value.contains('*') {
        return Err(AppError::invalid_input(
            field,
            "shared input paths are plain workspace-relative paths and do not support globs",
        ));
    }

    let path = Path::new(value);
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::invalid_input(
                    field,
                    "shared input paths must stay inside the workspace root",
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}
