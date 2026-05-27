//! Product error surface.

use std::{error::Error, fmt};

/// Result alias used by Toven library APIs.
pub type AppResult<T> = Result<T, AppError>;

/// Stable high-level error category.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ErrorCode {
    /// User-provided input or configuration is invalid.
    InvalidInput,
}

/// Error returned by Toven library APIs.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AppError {
    /// Stable high-level error category.
    pub code: ErrorCode,
    /// Input field or subsystem associated with the error.
    pub field: String,
    /// Human-readable diagnostic.
    pub message: String,
}

impl AppError {
    /// Build an invalid input error.
    pub fn invalid_input(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidInput,
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

impl Error for AppError {}

#[cfg(test)]
mod tests {
    use super::{AppError, ErrorCode};

    #[test]
    fn invalid_input_error_formats_field_and_message() {
        let error = AppError::invalid_input("template", "unknown placeholder");

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.to_string(), "template: unknown placeholder");
    }
}
