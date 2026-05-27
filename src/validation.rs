#![allow(clippy::redundant_pub_crate)]

use rskit_validation::Validator;

use crate::core::{AppError, AppResult, Template};

pub(crate) fn validate_name(field: impl Into<String>, value: &str) -> AppResult<()> {
    let field = field.into();
    Validator::new()
        .required(&field, value)
        .custom(
            value == value.trim(),
            &field,
            "cannot contain leading or trailing whitespace",
        )
        .validate()
}

pub(crate) fn validate_identifier(field: impl Into<String>, value: &str) -> AppResult<()> {
    let field = field.into();
    Validator::new()
        .required(&field, value)
        .custom(
            value == value.trim(),
            &field,
            "cannot contain leading or trailing whitespace",
        )
        .custom(
            !value.contains(['/', '\\', ':']) && value != "." && value != "..",
            &field,
            "cannot contain path separators or traversal markers",
        )
        .validate()
}

pub(crate) fn validate_command_template(
    field: impl Into<String>,
    values: &[String],
) -> AppResult<()> {
    let field = field.into();
    if values.is_empty() {
        return Err(AppError::invalid_input(
            field,
            "at least one argv item is required",
        ));
    }
    validate_templates(field, values)
}

pub(crate) fn validate_templates(field: impl Into<String>, values: &[String]) -> AppResult<()> {
    let field = field.into();
    for value in values {
        validate_template(&field, value)?;
    }
    Ok(())
}

pub(crate) fn validate_template(field: impl Into<String>, value: &str) -> AppResult<()> {
    let field = field.into();
    Template::parse(value).map_err(|error| {
        AppError::invalid_input(
            field,
            format!("invalid template '{value}': {}", error.message),
        )
    })?;
    Ok(())
}
