//! The discovery request — runtime context only; config is baked into the
//! [`ConfiguredAdapter`](crate::provider::ConfiguredAdapter).

use serde::{Deserialize, Serialize};
use toven_model::AbsPath;

/// Schema version of the discovery request/response envelope.
///
/// Bumped when the serialized shape changes; the out-of-process driver transport
/// validates it on both ends.
pub const DISCOVERY_SCHEMA_VERSION: u16 = 1;

/// Runtime context handed to a configured adapter's discovery pass.
///
/// The ecosystem config is **not** here — it was baked in by
/// [`Provider::configure`](crate::provider::Provider::configure). This carries
/// only what varies per run.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct DiscoverRequest {
    /// Envelope schema version ([`DISCOVERY_SCHEMA_VERSION`]).
    pub schema_version: u16,
    /// Absolute project root the adapter discovers under.
    pub project_root: AbsPath,
    /// Minimal extra context (e.g. an optional module filter).
    #[serde(default)]
    pub context: DiscoverContext,
}

impl DiscoverRequest {
    /// Construct a request stamped with the current schema version.
    #[must_use]
    pub fn new(project_root: AbsPath) -> Self {
        Self {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            project_root,
            context: DiscoverContext::default(),
        }
    }
}

/// Optional, minimal discovery context.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct DiscoverContext {
    /// When non-empty, restrict discovery to these module names (an adapter hint).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_filter: Vec<String>,
}

#[cfg(test)]
mod tests {
    use toven_model::AbsPath;

    use super::{DISCOVERY_SCHEMA_VERSION, DiscoverContext, DiscoverRequest};

    #[test]
    fn new_stamps_current_schema_and_empty_context() {
        let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute"));
        assert_eq!(request.schema_version, DISCOVERY_SCHEMA_VERSION);
        assert_eq!(request.context, DiscoverContext::default());
        assert!(request.context.module_filter.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute"));
        let json = toml::to_string(&request).expect("serialize");
        let back: DiscoverRequest = toml::from_str(&json).expect("deserialize");
        assert_eq!(request, back);
    }
}
