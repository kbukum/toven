/// The outcome of one scenario session.
#[derive(Debug, Clone)]
pub enum Report {
    /// A required toolchain was absent; the scenario was skipped green.
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
    /// Whether every executed step passed (or blessed), or the scenario was
    /// skipped.
    #[must_use]
    pub fn is_green(&self) -> bool {
        match self {
            Self::Skipped { .. } => true,
            Self::Completed { steps } => steps
                .iter()
                .all(|step| !matches!(step.status, StepStatus::Failed { .. })),
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
    /// The step's required toolchain was absent; the step was skipped green
    /// and later steps still ran.
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
