//! The PLAN inputs: what to plan, where, against which change baseline.

use toven_model::{AbsPath, ModuleRef, WorkspaceId};
use toven_ports::{BaselineSpec, TaskKind};

/// A user-named target for [`Selection::Explicit`].
///
/// A [`Module`](ModuleSelector::Module) names one module identity
/// (`ecosystem:name`); it activates every graph node with that identity (one node
/// in a single repo, or every member exposing it under an umbrella). A
/// [`Workspace`](ModuleSelector::Workspace) activates every module owned by a
/// discovered workspace. Toven resolves these against the discovered graph and
/// errors on a name that matches nothing — it never silently plans an empty run.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum ModuleSelector {
    /// One module identity (`ecosystem:name`), member-unscoped.
    Module(ModuleRef),
    /// Every module owned by a discovered workspace.
    Workspace(WorkspaceId),
}

/// How the active module set is selected before scheduling.
///
/// [`Selection::All`] activates every discovered module (a full `toven test`);
/// [`Selection::Changed`] runs the change mapper against the per-member
/// baselines resolved by the VCS reader set, falling back to the optional request
/// spec for members without their own configured baseline;
/// [`Selection::Explicit`] activates exactly the user-named modules/workspaces
/// (`--module`/`--workspace`), optionally expanded to their reverse dependents.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum Selection {
    /// Activate every discovered module.
    All,
    /// Activate only modules affected by changes since the resolved baseline.
    Changed(Option<BaselineSpec>),
    /// Activate only modules affected by an explicit set of changed paths.
    ///
    /// Paths are workspace-root-relative, with paths inside `.git` and paths
    /// ignored by the root repo already dropped. Watch mode feeds this per
    /// debounce batch; it maps the paths through the same change mapper as
    /// [`Selection::Changed`] but sources them from the filesystem watcher
    /// rather than a VCS baseline diff.
    ChangedPaths(Vec<String>),
    /// Activate exactly the named modules/workspaces resolved against the graph.
    Explicit {
        /// The user-named module/workspace targets to activate.
        targets: Vec<ModuleSelector>,
        /// Whether to also activate the reverse-dependents closure of the targets.
        include_dependents: bool,
    },
}

/// How the per-unit cache verdict is decided during PLAN.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
#[non_exhaustive]
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
