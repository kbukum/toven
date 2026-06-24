//! Explicit environment policy for command invocations.

use std::collections::BTreeMap;

/// Environment inheritance policy for a resolved command invocation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum InvocationEnvPolicy {
    /// Start from an empty environment and apply only explicit variables.
    ExplicitOnly,
    /// Inherit the parent environment, then apply explicit overrides.
    InheritParent,
}

/// Environment supplied to a command invocation.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InvocationEnvironment {
    /// Whether parent environment variables are inherited.
    pub policy: InvocationEnvPolicy,
    /// Explicit environment variables.
    pub vars: BTreeMap<String, String>,
}

impl InvocationEnvironment {
    /// Empty environment with no inherited variables.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            policy: InvocationEnvPolicy::ExplicitOnly,
            vars: BTreeMap::new(),
        }
    }

    /// Explicit variables only.
    #[must_use]
    pub const fn explicit(vars: BTreeMap<String, String>) -> Self {
        Self {
            policy: InvocationEnvPolicy::ExplicitOnly,
            vars,
        }
    }

    /// Parent environment inheritance, explicitly opted into.
    #[must_use]
    pub const fn inherit_parent(vars: BTreeMap<String, String>) -> Self {
        Self {
            policy: InvocationEnvPolicy::InheritParent,
            vars,
        }
    }
}

impl Default for InvocationEnvironment {
    fn default() -> Self {
        Self::empty()
    }
}
