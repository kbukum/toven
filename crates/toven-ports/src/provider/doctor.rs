//! [`DoctorScope`] — what `toven doctor` audits for one configured ecosystem.

use serde::{Deserialize, Serialize};
use toven_model::{RepoPath, ToolchainTag};

/// What `toven doctor` audits for one configured ecosystem: the toolchains
/// whose task probes the tool audit runs, and the ecosystem's own health
/// checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DoctorScope {
    /// The toolchain tags whose per-task probes the tool audit runs.
    pub toolchains: Vec<ToolchainTag>,
    /// The ecosystem's health checks, in report order.
    pub checks: Vec<EcosystemCheck>,
}

impl DoctorScope {
    /// A scope auditing `toolchains` with no ecosystem checks.
    #[must_use]
    pub const fn new(toolchains: Vec<ToolchainTag>) -> Self {
        Self {
            toolchains,
            checks: Vec::new(),
        }
    }

    /// Add one health check.
    #[must_use]
    pub fn with_check(mut self, check: EcosystemCheck) -> Self {
        self.checks.push(check);
        self
    }
}

/// One ecosystem health check.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct EcosystemCheck {
    /// The check id, unique within the ecosystem (`web/install`). The engine
    /// prefixes the ecosystem id when it reports the check.
    pub id: String,
    /// The verdict, or the command that decides it.
    pub verdict: CheckVerdict,
    /// What to run or do when the check fails.
    pub fix: Option<String>,
}

impl EcosystemCheck {
    /// A check with `id` and `verdict`, and no fix hint.
    #[must_use]
    pub fn new(id: impl Into<String>, verdict: CheckVerdict) -> Self {
        Self {
            id: id.into(),
            verdict,
            fix: None,
        }
    }

    /// Attach the fix hint reported when the check fails.
    #[must_use]
    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// How an [`EcosystemCheck`] is decided.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum CheckVerdict {
    /// The adapter decided the check passed (an offline file check).
    Passed,
    /// The adapter decided the check failed, for this reason.
    Failed(String),
    /// A command the engine runs (argv, no shell) in the repo-relative `dir`.
    /// It must exit 0 and, with `version`, print that text.
    Command {
        /// The command line.
        argv: Vec<String>,
        /// The repo-relative working directory.
        dir: RepoPath,
        /// Text the output must contain, such as a pinned version.
        version: Option<String>,
    },
}
