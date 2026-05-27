use rskit_validation::Validator;

pub(crate) fn validate_name(field: impl AsRef<str>, value: &str) -> crate::core::AppResult<()> {
    let field = field.as_ref();
    Validator::new()
        .required(field, value)
        .custom(
            value == value.trim(),
            field,
            "cannot contain leading or trailing whitespace",
        )
        .validate()
}

pub(crate) fn validate_identifier(
    field: impl AsRef<str>,
    value: &str,
) -> crate::core::AppResult<()> {
    let field = field.as_ref();
    Validator::new()
        .required(field, value)
        .custom(
            value == value.trim(),
            field,
            "cannot contain leading or trailing whitespace",
        )
        .custom(
            !value.contains(['/', '\\', ':']) && value != "." && value != "..",
            field,
            "cannot contain path separators or traversal markers",
        )
        .validate()
}

pub(crate) fn validate_command_template(
    field: impl AsRef<str>,
    values: &[String],
) -> crate::core::AppResult<()> {
    let field = field.as_ref();
    if values.is_empty() {
        return Err(crate::core::AppError::invalid_input(
            field,
            "at least one argv item is required",
        ));
    }
    validate_templates(field, values)
}

pub(crate) fn validate_templates(
    field: impl AsRef<str>,
    values: &[String],
) -> crate::core::AppResult<()> {
    let field = field.as_ref();
    for value in values {
        validate_template(field, value)?;
    }
    Ok(())
}

pub(crate) fn validate_template(field: impl AsRef<str>, value: &str) -> crate::core::AppResult<()> {
    let field = field.as_ref();
    crate::core::Template::parse(value).map_err(|error| {
        crate::core::AppError::invalid_input(
            field,
            format!("invalid template '{value}': {}", error.message),
        )
    })?;
    Ok(())
}
