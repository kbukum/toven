//! The config-less scaffold exchange — the federated half of `toven generate`.
//!
//! Unlike the [`envelope`](super::envelope) RPC mirror (which configures an
//! adapter from an `[ecosystems.<id>]` subtree before answering port calls),
//! scaffolding runs **before any config exists**. So it is a distinct, one-shot
//! exchange: the umbrella sends a [`ScaffoldRequest`] naming the project root,
//! the driven `toven-<eco> __scaffold` process self-detects every ecosystem it
//! serves and answers with a single [`ScaffoldOutcome`] carrying the detected
//! [`EcosystemFragment`]s (or a typed error). It reuses the same length-delimited
//! [`codec`](super::codec) framing as the port-call protocol.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use toven_ports::EcosystemFragment;

use super::envelope::{ENVELOPE_SCHEMA_VERSION, WireError};

/// The umbrella's config-less scaffold probe: "what do you detect under here?".
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct ScaffoldRequest {
    /// Envelope schema version ([`ENVELOPE_SCHEMA_VERSION`]).
    pub schema_version: u16,
    /// The project root the driver self-detects its ecosystems under.
    pub project_root: PathBuf,
}

impl ScaffoldRequest {
    /// Build a scaffold request for `project_root` at the current schema version.
    #[must_use]
    pub const fn new(project_root: PathBuf) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            project_root,
        }
    }
}

/// A driven `__scaffold` process's reply: the detected fragments, or a failure.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum ScaffoldOutcome {
    /// Every `[ecosystems.<id>]` fragment the driver's providers detected
    /// (possibly empty when the driver serves no ecosystem under the root).
    Fragments(Vec<EcosystemFragment>),
    /// The driver could not complete detection; carries a typed, displayable cause.
    Error(WireError),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use toml::Table;
    use toven_ports::EcosystemFragment;

    use super::super::envelope::WireError;
    use super::{ScaffoldOutcome, ScaffoldRequest};

    #[test]
    fn request_round_trips_through_json() {
        let request = ScaffoldRequest::new(PathBuf::from("/repo"));
        let json = serde_json::to_string(&request).expect("serialize");
        let back: ScaffoldRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(request, back);
    }

    #[test]
    fn fragments_outcome_round_trips_through_json() {
        let ecosystem = toven_model::EcosystemId::new("rust").expect("valid id");
        let outcome =
            ScaffoldOutcome::Fragments(vec![EcosystemFragment::new(ecosystem, Table::new())]);
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: ScaffoldOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }

    #[test]
    fn error_outcome_round_trips_through_json() {
        let outcome = ScaffoldOutcome::Error(WireError::new(
            rskit_errors::ErrorCode::Internal.as_str(),
            "boom",
        ));
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: ScaffoldOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }
}
