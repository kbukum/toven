/// The outcome of one scenario session.
#[derive(Debug, Clone)]
pub enum Report {
    /// A required toolchain was absent; the scenario was skipped.
    Skipped {
        /// The first missing toolchain program (e.g. `cargo`).
        tool: String,
    },
    /// The session ran; per-step outcomes in scenario order. Execution stops
    /// at the first failed step, so at most the last entry is `Failed`.
    Completed {
        /// One outcome per executed step, in order.
        steps: Vec<StepOutcome>,
    },
}

impl Report {
    /// Map the report onto a pass/fail verdict.
    ///
    /// A failed step always fails, carrying its diff. A skip (the whole
    /// scenario or one step, for a missing toolchain) passes under
    /// [`SkipPolicy::Allow`] and fails under [`SkipPolicy::Fail`], so a gate
    /// that must exercise every scenario cannot pass on an under-provisioned
    /// host.
    ///
    /// # Errors
    ///
    /// Returns the failure message naming the failed or skipped step.
    pub fn verdict(&self, policy: SkipPolicy) -> Result<(), String> {
        if let Some(step) = self.failure()
            && let StepStatus::Failed { message } = &step.status
        {
            return Err(format!("step '{}' failed:\n{message}", step.id));
        }
        if policy == SkipPolicy::Allow {
            return Ok(());
        }
        match self {
            Self::Skipped { tool } => Err(format!(
                "scenario skipped: requires `{tool}` which is not on PATH"
            )),
            Self::Completed { steps } => steps
                .iter()
                .find_map(|step| match &step.status {
                    StepStatus::Skipped { tool } => Some(Err(format!(
                        "step '{}' skipped: requires `{tool}` which is not on PATH",
                        step.id
                    ))),
                    _ => None,
                })
                .unwrap_or(Ok(())),
        }
    }

    /// The failed step, if any.
    #[must_use]
    pub fn failure(&self) -> Option<&StepOutcome> {
        match self {
            Self::Skipped { .. } => None,
            Self::Completed { steps } => steps
                .iter()
                .find(|step| matches!(step.status, StepStatus::Failed { .. })),
        }
    }
}

/// Whether a skipped scenario or step (missing toolchain) passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkipPolicy {
    /// A skip passes; the default for local and sharded test runs.
    #[default]
    Allow,
    /// A skip fails; for gates that must exercise every selected scenario.
    Fail,
}

impl SkipPolicy {
    /// The environment switch that selects [`SkipPolicy::Fail`] when set to `1`.
    pub const ENV: &'static str = "TOVEN_GOLDEN_REQUIRE_TOOLS";

    /// Read the policy from [`Self::ENV`].
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_value(std::env::var(Self::ENV).ok().as_deref())
    }

    /// Parse the policy from the raw [`Self::ENV`] value.
    #[must_use]
    pub fn from_value(value: Option<&str>) -> Self {
        if value == Some("1") {
            Self::Fail
        } else {
            Self::Allow
        }
    }
}

/// The outcome of one executed step.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    /// The step's stable id.
    pub id: String,
    /// What happened.
    pub status: StepStatus,
}

/// What happened to one executed step.
#[derive(Debug, Clone)]
pub enum StepStatus {
    /// Exit code, every asserted stream, and every effect held.
    Passed,
    /// At least one golden was regenerated (bless mode); everything else held.
    Blessed,
    /// The step's required toolchain was absent; the step was skipped and
    /// later steps still ran.
    Skipped {
        /// The first missing toolchain program (e.g. `cargo-cyclonedx`).
        tool: String,
    },
    /// The step failed; `message` carries the diff or drift, naming the step
    /// and the surface that failed.
    Failed {
        /// Human-readable failure detail (unified diff, exit drift, effect).
        message: String,
    },
}
