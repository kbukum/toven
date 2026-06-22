//! `[groups.<name>]` — federation-level module membership + guardrails.

use toven_model::EcosystemId;

use serde::{Deserialize, Serialize};

/// A reserved `[groups.<name>]` section.
///
/// Membership is human-declared (`modules`); group dependencies are *derived*
/// from the real module graph (no manual `depends_on`, so no drift). Guardrails
/// are declared edges the engine enforces. Groups are federation-level and may
/// span ecosystems; semantic resolution of the listed refs happens later in the
/// engine Graph phase.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupConfig {
    /// Optional default ecosystem, letting `modules` use bare (unqualified) names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<EcosystemId>,
    /// Member modules: bare names (with `ecosystem` set) or `ecosystem:module`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Declared, engine-enforced dependency guardrails.
    #[serde(default, skip_serializing_if = "Guardrails::is_default")]
    pub guardrails: Guardrails,
}

/// Declared dependency guardrails for a group (engine-enforced).
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Guardrails {
    /// Fully-qualified `ecosystem:module` edges that must NOT be depended on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbid: Vec<String>,
    /// Optional allowlist of fully-qualified `ecosystem:module` edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
}

impl Guardrails {
    /// Whether both lists are empty (so the section can be skipped on serialize).
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.forbid.is_empty() && self.allow.is_empty()
    }
}
