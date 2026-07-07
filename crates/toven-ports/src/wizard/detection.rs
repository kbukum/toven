//! [`Detection`] — an adapter's probe result for one ecosystem under a root.

use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;

/// The result of a [`Provider::detect`](crate::provider::Provider::detect) probe.
///
/// Carries the ecosystem that applies under a project root plus the opaque,
/// adapter-owned facts it needs later at
/// [`render`](crate::provider::Provider::render) time.
///
/// `facts` is the adapter's own serializable data (detected manifests, whether a
/// preferred test runner is available, …), carried through the wizard unchanged;
/// core never inspects it. Keeping it a raw [`Table`] means it survives the
/// framed federation transport intact — a driver's detection crosses to the
/// umbrella and its answers cross back without core knowing the shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// The ecosystem this detection is for (`[ecosystems.<id>]` key).
    pub ecosystem: EcosystemId,
    /// Opaque, adapter-owned probe facts, consumed at `render` time.
    pub facts: Table,
}

impl Detection {
    /// Construct a detection for `ecosystem` carrying the adapter's `facts`.
    #[must_use]
    pub const fn new(ecosystem: EcosystemId, facts: Table) -> Self {
        Self { ecosystem, facts }
    }

    /// A detection with no additional facts (an empty table).
    #[must_use]
    pub fn bare(ecosystem: EcosystemId) -> Self {
        Self::new(ecosystem, Table::new())
    }
}

#[cfg(test)]
mod tests {
    use super::Detection;
    use toml::{Table, Value};
    use toven_model::EcosystemId;

    #[test]
    fn round_trips_through_json() {
        let mut facts = Table::new();
        facts.insert("nextest".to_string(), Value::Boolean(true));
        let detection = Detection::new(EcosystemId::new("rust").expect("id"), facts);
        let json = serde_json::to_string(&detection).expect("serialize");
        let back: Detection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(detection, back);
    }

    #[test]
    fn bare_carries_no_facts() {
        let detection = Detection::bare(EcosystemId::new("go").expect("id"));
        assert!(detection.facts.is_empty());
    }
}
