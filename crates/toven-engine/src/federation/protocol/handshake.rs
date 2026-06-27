//! Protocol version negotiation and typed driver-failure classification.
//!
//! The umbrella requires a **MAJOR match** and tolerates additive **MINOR**
//! differences (Decision 5). A resolved driver that fails compatibility — or
//! crashes, times out, or emits a malformed frame — is a hard PLAN error
//! (classified by [`DriverFault`]); only an *absent* driver is warn + skip, and
//! that distinction is made earlier, in [`resolve`](super::super::resolve).

use rskit_errors::{AppError, ErrorCode};
use rskit_version::semver::{Version, parse_version};

/// The protocol version this build of Toven speaks.
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Parse the compiled-in [`PROTOCOL_VERSION`] into a semver [`Version`].
///
/// # Panics
/// Never in practice: [`PROTOCOL_VERSION`] is a compile-time constant verified by
/// the `protocol_version_is_valid_semver` test. The fallback keeps this total
/// without an `unwrap` on the runtime path.
#[must_use]
pub fn protocol_version() -> Version {
    parse_version(PROTOCOL_VERSION).unwrap_or(Version::new(1, 0, 0))
}

/// A typed classification of why a *resolved* driver could not be used.
///
/// Every variant is a hard PLAN error: a partial federation would silently
/// corrupt the cross-language affected closure, so a broken-but-present driver
/// must abort rather than degrade.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum DriverFault {
    /// The driver speaks an incompatible protocol major version.
    IncompatibleProtocol {
        /// The major version the umbrella requires.
        required: u64,
        /// The major version the driver offered.
        offered: u64,
    },
    /// The driver could not be spawned (missing binary, exec failure).
    Spawn(String),
    /// A frame could not be encoded, decoded, or was truncated.
    Malformed(String),
    /// An RPC exceeded its deadline; the driver was terminated.
    Timeout,
    /// The transport failed (broken pipe, unexpected EOF mid-session).
    Transport(String),
    /// The driver answered with a typed remote error.
    Remote {
        /// Remote error-code label.
        code: String,
        /// Remote error message.
        message: String,
    },
}

impl DriverFault {
    /// Render this fault as a typed [`AppError`] tagged with the ecosystem id.
    #[must_use]
    pub fn into_app_error(self, ecosystem: &str) -> AppError {
        let (code, detail) = match self {
            Self::IncompatibleProtocol { required, offered } => (
                ErrorCode::Conflict,
                format!(
                    "driver speaks protocol major v{offered}, but this Toven requires major v{required}"
                ),
            ),
            Self::Spawn(message) => (
                ErrorCode::ServiceUnavailable,
                format!("could not spawn driver: {message}"),
            ),
            Self::Malformed(message) => (
                ErrorCode::InvalidInput,
                format!("malformed driver frame: {message}"),
            ),
            Self::Timeout => (
                ErrorCode::Timeout,
                "driver did not respond before its deadline".to_string(),
            ),
            Self::Transport(message) => (
                ErrorCode::ServiceUnavailable,
                format!("driver transport failure: {message}"),
            ),
            Self::Remote { code, message } => {
                let error_code = ErrorCode::from_wire(&code).unwrap_or(ErrorCode::Internal);
                (error_code, format!("driver reported {code}: {message}"))
            }
        };
        AppError::new(
            code,
            format!("ecosystem '{ecosystem}' driver failed: {detail}"),
        )
    }
}

/// Verify a driver's offered protocol version against the umbrella's.
///
/// Compatible when the major versions match (additive minor/patch differences
/// are tolerated in either direction). Returns the classified
/// [`DriverFault::IncompatibleProtocol`] otherwise.
///
/// # Errors
/// Returns [`DriverFault::Malformed`] if `offered` is not valid semver, or
/// [`DriverFault::IncompatibleProtocol`] on a major mismatch.
pub fn negotiate(required: &Version, offered: &str) -> Result<(), DriverFault> {
    let offered = parse_version(offered).ok_or_else(|| {
        DriverFault::Malformed(format!("offered protocol '{offered}' is not semver"))
    })?;
    if offered.major == required.major {
        Ok(())
    } else {
        Err(DriverFault::IncompatibleProtocol {
            required: required.major,
            offered: offered.major,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{DriverFault, PROTOCOL_VERSION, negotiate, protocol_version};
    use rskit_version::semver::parse_version;

    #[test]
    fn protocol_version_is_valid_semver() {
        assert!(parse_version(PROTOCOL_VERSION).is_some());
        assert_eq!(protocol_version().major, 1);
    }

    #[test]
    fn matching_major_is_accepted() {
        assert!(negotiate(&protocol_version(), "1.4.2").is_ok());
        assert!(negotiate(&protocol_version(), "1.0.0").is_ok());
    }

    #[test]
    fn mismatched_major_is_rejected() {
        let fault = negotiate(&protocol_version(), "2.0.0").expect_err("incompatible");
        assert!(matches!(
            fault,
            DriverFault::IncompatibleProtocol {
                required: 1,
                offered: 2
            }
        ));
    }

    #[test]
    fn non_semver_offer_is_malformed() {
        assert!(matches!(
            negotiate(&protocol_version(), "not-a-version"),
            Err(DriverFault::Malformed(_))
        ));
    }

    #[test]
    fn incompatible_fault_maps_to_failed_precondition() {
        let error = DriverFault::IncompatibleProtocol {
            required: 1,
            offered: 2,
        }
        .into_app_error("go");
        assert_eq!(error.code(), rskit_errors::ErrorCode::Conflict);
    }

    #[test]
    fn remote_fault_preserves_wire_error_code() {
        // A real `WireError` carries the `as_str` form (e.g. "NOT_FOUND"), which is
        // exactly what `from_wire` round-trips back into a typed code.
        let error = DriverFault::Remote {
            code: rskit_errors::ErrorCode::NotFound.as_str().to_string(),
            message: "module not found".to_string(),
        }
        .into_app_error("go");
        assert_eq!(error.code(), rskit_errors::ErrorCode::NotFound);
    }

    #[test]
    fn remote_fault_falls_back_to_internal_for_unknown_code() {
        let error = DriverFault::Remote {
            code: "SomethingUnknown".to_string(),
            message: "unusual error".to_string(),
        }
        .into_app_error("go");
        assert_eq!(error.code(), rskit_errors::ErrorCode::Internal);
    }
}
