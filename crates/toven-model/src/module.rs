//! Discovered module model (identity + topology, language-agnostic).

use serde::{Deserialize, Serialize};

use crate::identity::{MemberId, ModuleKey, ModuleRef, RepoPath, WorkspaceId};

/// A discovered module, independent of language-specific manifests.
///
/// Dependencies live in a separate [`Edge`](crate::Edge) list rather than
/// inline, so federation is a plain union of module and edge sets and
/// cross-ecosystem overlay edges share one uniform edge set.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct Module {
    /// Stable identity (`ecosystem:name`).
    pub id: ModuleRef,
    /// Package/crate name used by command templates (may differ from
    /// `id.name`).
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
    /// Serialization resource group: units sharing this label are serialized by
    /// the executor because they contend on one resource (e.g. a shared
    /// `target/` directory). Adapter-set during discovery; `None` leaves the
    /// unit unguarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
    /// Whether this module has an executable target a `run`-kind task can
    /// launch. Adapter-set during discovery; defaults `true` (assume runnable)
    /// so only an adapter that can prove a module is library-only (no binary)
    /// excludes it from persistent `run` units. Consumed by the scheduler, not
    /// identity.
    #[serde(default = "default_runnable", skip_serializing_if = "is_runnable")]
    pub runnable: bool,
}

/// Serde default for [`Module::runnable`]: assume a module is runnable unless
/// the discovering adapter proves otherwise.
const fn default_runnable() -> bool {
    true
}

/// Serde skip helper: omit `runnable` from output when it holds its default.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_runnable(value: &bool) -> bool {
    *value
}

impl Module {
    /// Construct a module with only the required identity + root, leaving
    /// optional fields empty. Optional fields are set directly by the discovery
    /// adapter.
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
            resource_group: None,
            runnable: true,
        }
    }

    /// The graph key for this module: its identity scoped by its `member`.
    ///
    /// `member` is `None` for a single-repo module, so the key renders and
    /// orders identically to its [`ModuleRef`] there; under a cross-repo
    /// umbrella the member qualifier keeps two members' same `ecosystem:name`
    /// distinct.
    #[must_use]
    pub fn key(&self) -> ModuleKey {
        ModuleKey::new(self.member.clone(), self.id.clone())
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
        // `runnable` defaults true and is skipped when it holds that default.
        assert!(!json.contains("runnable"));
        let back: Module = serde_json::from_str(&json).unwrap();
        assert_eq!(module, back);
        assert!(back.runnable);
    }

    #[test]
    fn a_non_runnable_module_round_trips_the_flag() {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            RepoPath::new("core/errors").unwrap(),
        );
        module.runnable = false;
        let json = serde_json::to_string(&module).unwrap();
        assert!(json.contains("runnable"));
        let back: Module = serde_json::from_str(&json).unwrap();
        assert!(!back.runnable);
        assert_eq!(module, back);
    }
}
