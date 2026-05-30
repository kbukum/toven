#![allow(clippy::redundant_pub_crate)]

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
