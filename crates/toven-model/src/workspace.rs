//! Workspace descriptor and the toolchain identity it carries.

use serde::{Deserialize, Serialize};

use crate::identity::{RepoPath, WorkspaceId};

/// Driver identity for a workspace, folded into every unit's cache key.
///
/// `version` is an adapter-composed **opaque** string that changes iff the
/// cache-significant toolchain identity changes (e.g. `"rustc 1.94.0 (cargo
/// 1.94.0)"`). It is `None` until the planner resolves it once per active
/// workspace.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct ToolchainTag {
    /// Driver name (`cargo`, `pnpm`) — used for display and argv program choice.
    pub tool: String,
    /// Opaque, adapter-composed version identity; `None` until resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ToolchainTag {
    /// Construct an unresolved tag carrying only the driver name.
    #[must_use]
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            version: None,
        }
    }

    /// Return a copy of this tag with `version` resolved.
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

/// A discovery unit (one Cargo workspace, one `go.work`) that drives multi-tooling.
///
/// Promoted from a bare string to an object because the resolved driver+version
/// is needed by argv rendering, the cache key, and resource grouping.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct Workspace {
    /// Stable workspace identity.
    pub id: WorkspaceId,
    /// Repo-relative workspace root.
    pub root: RepoPath,
    /// Resolved driver identity for this workspace.
    pub toolchain: ToolchainTag,
    /// Workspace-wide blast-radius input globs: a change to any path matching one
    /// of these activates every member of the workspace (e.g. a shared
    /// `Cargo.lock`). Adapter-set during discovery; empty leaves only per-module
    /// source patterns to drive change detection.
    #[serde(default)]
    pub blast_radius: Vec<String>,
}

impl Workspace {
    /// Construct a workspace with an empty blast radius.
    #[must_use]
    pub const fn new(id: WorkspaceId, root: RepoPath, toolchain: ToolchainTag) -> Self {
        Self {
            id,
            root,
            toolchain,
            blast_radius: Vec::new(),
        }
    }
}
