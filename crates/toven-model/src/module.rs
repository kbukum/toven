//! Discovered module model (identity + topology, language-agnostic).

use serde::{Deserialize, Serialize};

use crate::{
    identity::{MemberId, ModuleRef, RepoPath, WorkspaceId},
    metadata::Metadata,
};

/// A discovered module, independent of language-specific manifests.
///
/// Dependencies live in a separate [`Edge`](crate::Edge) list rather than inline,
/// so federation is a plain union of module and edge sets and cross-ecosystem
/// overlay edges share one uniform edge set.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct Module {
    /// Stable identity (`ecosystem:name`).
    pub id: ModuleRef,
    /// Package/crate name used by command templates (may differ from `id.name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Repo-relative module root.
    pub root: RepoPath,
    /// Repo-relative manifest path (`Cargo.toml`, `go.mod`, `package.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<RepoPath>,
    /// Discovery unit that owns this module (metadata, not identity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// Repository member in a cross-repo federation (`None` = single repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberId>,
    /// Repo-relative globs used for change detection.
    #[serde(default)]
    pub source_patterns: Vec<String>,
    /// Freeform adapter data (feeds topology/release/report).
    #[serde(default)]
    pub metadata: Metadata,
}

impl Module {
    /// Construct a module with only the required identity + root, leaving optional
    /// fields empty. Optional fields are set directly by the discovery adapter.
    #[must_use]
    pub const fn new(id: ModuleRef, root: RepoPath) -> Self {
        Self {
            id,
            package: None,
            root,
            manifest: None,
            workspace: None,
            member: None,
            source_patterns: Vec::new(),
            metadata: Metadata::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Module;
    use crate::identity::{EcosystemId, ModuleRef, RepoPath};

    #[test]
    fn serde_round_trip_omits_empty_options() {
        let module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            RepoPath::new("core/errors").unwrap(),
        );
        let json = serde_json::to_string(&module).unwrap();
        assert!(!json.contains("package"));
        let back: Module = serde_json::from_str(&json).unwrap();
        assert_eq!(module, back);
    }
}
