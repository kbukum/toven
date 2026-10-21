//! `[groups.<name>]` — federation-level module membership + guardrails.

use std::collections::BTreeMap;

use toven_model::EcosystemId;
use toven_ports::{RunStrategy, TaskOverride};

use serde::{Deserialize, Serialize};

/// A reserved `[groups.<name>]` section.
///
/// Membership is human-declared (`modules`); group dependencies are *derived*
/// from the real module graph (no manual `depends_on`, so no drift). Guardrails
/// are declared rules the engine enforces against those real edges, including
/// which other groups the members may depend on (`allow_groups`). Groups are federation-level and may
/// span ecosystems; semantic resolution of the listed refs happens later in the
/// engine Graph phase.
///
/// A group may additionally carry **scope overrides** — a per-group `tasks` map
/// and `run_strategy` that layer on top of the ecosystem/adapter defaults for
/// the group's members only. The merge order is `adapter default → ecosystem
/// [tasks] → group [tasks]`; a module reached by two groups that both override
/// the same task (or `run_strategy`) is a hard error (see the Graph phase), so
/// overrides stay explicit and fail closed.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupConfig {
    /// What the group is, for people and generated docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional default ecosystem, letting `modules` use bare (unqualified)
    /// names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<EcosystemId>,
    /// Member modules: bare names (with `ecosystem` set) or `ecosystem:module`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Group-scoped wave-ordering override applied to the group's members only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_strategy: Option<RunStrategy>,
    /// Group-scoped task overrides, keyed by task name, field-merged onto the
    /// ecosystem/adapter default for the group's members only.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, TaskOverride>,
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
    /// Groups (declared in the same scope) the members may depend on. When
    /// set, a dependency leaving the group must land in one of these groups or
    /// in `allow`; an empty list allows no other group. Unset leaves group
    /// dependencies unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_groups: Option<Vec<String>>,
}

impl Guardrails {
    /// Whether both lists are empty (so the section can be skipped on
    /// serialize).
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.forbid.is_empty() && self.allow.is_empty() && self.allow_groups.is_none()
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::RunStrategy;

    use super::GroupConfig;

    #[test]
    fn round_trips_scope_overrides_through_toml() {
        let toml = r#"
            ecosystem = "rust"
            modules = ["rust:it-a", "rust:it-b"]
            run_strategy = "unordered"

            [tasks.test]
            argv = ["cargo", "nextest", "run", "--profile", "ci"]
            cache_args = true
        "#;
        let group: GroupConfig = toml::from_str(toml).expect("parse");
        assert_eq!(group.run_strategy, Some(RunStrategy::Unordered));
        let test = group.tasks.get("test").expect("test override present");
        assert_eq!(
            test.argv.as_deref().unwrap(),
            ["cargo", "nextest", "run", "--profile", "ci"]
        );
        assert_eq!(test.cache_args, Some(true));

        let serialized = toml::to_string(&group).expect("serialize");
        let back: GroupConfig = toml::from_str(&serialized).expect("re-parse");
        assert_eq!(group, back);
    }

    #[test]
    fn unknown_group_key_is_rejected() {
        let error = toml::from_str::<GroupConfig>("bogus = true").expect_err("strict parse");
        assert!(error.to_string().contains("bogus"), "{error}");
    }

    #[test]
    fn unknown_task_override_key_is_rejected() {
        let toml = r"
            [tasks.test]
            bogus = true
        ";
        let error = toml::from_str::<GroupConfig>(toml).expect_err("strict parse");
        assert!(error.to_string().contains("bogus"), "{error}");
    }

    #[test]
    fn group_task_cannot_override_ecosystem_module_scope() {
        let error = toml::from_str::<GroupConfig>("[tasks.test]\nmodules = [\"app\"]")
            .expect_err("module scope belongs to the ecosystem task");
        assert!(error.to_string().contains("modules"), "{error}");
    }
}
