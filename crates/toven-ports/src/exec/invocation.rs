//! [`Invocation`] — a fully resolved command the APPLY exec layer runs.

use std::time::Duration;

use toven_model::{ExecutionReadiness, ExecutionUnit};

use super::InvocationEnvironment;

/// One resolved command ready to execute.
///
/// The argv is already fully rendered during PLAN (every template variable
/// resolved, passthrough spliced); the runner never rewrites it — argv is
/// user-owned. The runner only supplies the working directory and environment
/// policy it was constructed with.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Invocation {
    /// Stable unit id this invocation belongs to (labels raw output + events).
    pub unit_id: String,
    /// Fully rendered argument vector (`argv[0]` is the program).
    pub argv: Vec<String>,
    /// Whether this invocation starts a long-lived (persistent) process.
    pub persistent: bool,
    /// Readiness policy for persistent invocations.
    pub readiness: ExecutionReadiness,
    /// Readiness timeout for persistent invocations.
    pub readiness_timeout: Duration,
    /// Explicit environment policy for the invocation.
    pub environment: InvocationEnvironment,
}

impl Invocation {
    /// Construct an invocation for `unit_id` from a rendered `argv`.
    #[must_use]
    pub fn new(unit_id: impl Into<String>, argv: Vec<String>) -> Self {
        Self {
            unit_id: unit_id.into(),
            argv,
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            environment: InvocationEnvironment::default(),
        }
    }

    /// Construct an invocation from a planned execution unit using a single, explicit
    /// environment policy.
    #[must_use]
    pub fn from_unit(unit: &ExecutionUnit, environment: InvocationEnvironment) -> Self {
        Self {
            unit_id: unit.id.clone(),
            argv: unit.argv.clone(),
            persistent: unit.persistent,
            readiness: unit.readiness.clone(),
            readiness_timeout: unit.readiness_timeout,
            environment,
        }
    }

    /// Mark this invocation as starting a persistent process.
    #[must_use]
    pub const fn with_persistent(mut self, persistent: bool) -> Self {
        self.persistent = persistent;
        self
    }

    /// Set the persistent readiness policy.
    #[must_use]
    pub fn with_readiness(mut self, readiness: ExecutionReadiness) -> Self {
        self.readiness = readiness;
        self
    }

    /// Set the persistent readiness timeout.
    #[must_use]
    pub const fn with_readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }

    /// Set the command environment policy.
    #[must_use]
    pub fn with_environment(mut self, environment: InvocationEnvironment) -> Self {
        self.environment = environment;
        self
    }
}
