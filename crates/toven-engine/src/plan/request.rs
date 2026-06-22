//! The PLAN inputs: what to plan, where, against which change baseline.

use toven_model::AbsPath;
use toven_ports::{BaselineSpec, TaskKind};

/// How the active module set is selected before scheduling.
///
/// [`Selection::All`] activates every discovered module (a full `toven test`);
/// [`Selection::Changed`] runs the the change mapper against the
/// changed paths the [`VcsReader`](toven_ports::VcsReader) reports for `spec`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Selection {
    /// Activate every discovered module.
    All,
    /// Activate only the modules affected by the changes since `spec`.
    Changed(BaselineSpec),
}

/// How the per-unit cache verdict is decided during PLAN.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum CacheMode {
    /// Read existing records and write new ones (the default).
    #[default]
    ReadWrite,
    /// Ignore existing records: every unit is [`Forced`](toven_model::CacheVerdict::Forced).
    Force,
    /// Bypass the cache entirely: every unit is
    /// [`Disabled`](toven_model::CacheVerdict::Disabled).
    Disabled,
}

/// The immutable inputs to one PLAN run.
///
/// User-owned argv (`passthrough`) is carried verbatim and never rewritten; the
/// planner only validates and expands it at the `{args}` splice point.
#[derive(Debug, Clone)]
pub struct PlanRequest {
    /// Stable run identifier, echoed into the emitted [`Event`](toven_model::Event)s.
    pub run_id: String,
    /// Human-facing project name (from `[project].name`), echoed into events.
    pub project: String,
    /// The task kind to plan across the federation (e.g. [`TaskKind::Test`]).
    pub intent: TaskKind,
    /// Absolute project root discovery and source hashing resolve against.
    pub project_root: AbsPath,
    /// User passthrough args, spliced verbatim at each task's `{args}` point.
    pub passthrough: Vec<String>,
    /// How the active module set is selected.
    pub selection: Selection,
    /// How cache verdicts are decided.
    pub cache_mode: CacheMode,
}

impl PlanRequest {
    /// Construct a request for `intent` rooted at `project_root`, defaulting to a
    /// full [`Selection::All`] run with [`CacheMode::ReadWrite`].
    #[must_use]
    pub fn new(
        run_id: impl Into<String>,
        project: impl Into<String>,
        intent: TaskKind,
        project_root: AbsPath,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            project: project.into(),
            intent,
            project_root,
            passthrough: Vec::new(),
            selection: Selection::All,
            cache_mode: CacheMode::ReadWrite,
        }
    }

    /// Replace the passthrough args spliced at `{args}`.
    #[must_use]
    pub fn with_passthrough(mut self, passthrough: Vec<String>) -> Self {
        self.passthrough = passthrough;
        self
    }

    /// Replace the active-set selection.
    #[must_use]
    pub fn with_selection(mut self, selection: Selection) -> Self {
        self.selection = selection;
        self
    }

    /// Replace the cache mode.
    #[must_use]
    pub const fn with_cache_mode(mut self, cache_mode: CacheMode) -> Self {
        self.cache_mode = cache_mode;
        self
    }
}
