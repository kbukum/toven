//! Discovered module model (identity + topology, language-agnostic).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
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
    /// What this module provides (`bin`, `script:test`). Adapter-set during
    /// discovery and matched against a task's required capabilities: a module
    /// lacking one is not eligible for that task. Opaque to the model.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<Capability>,
    /// Whether discovery represents every mutable dependency in the source
    /// graph. False or absent metadata disables caching for this module and
    /// its consumers.
    #[serde(default)]
    pub cache_inputs_complete: bool,
    /// Project-relative files or directories outside the module graph that
    /// this module builds from (for example a Go `replace` to a sibling
    /// checkout, or a packed JavaScript tarball). They may start with `..`.
    /// Their content joins the module's source identity, and because version
    /// control cannot see them the module is always affected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_roots: Vec<String>,
    /// Subtrees under `root`, relative to it, that belong to another module
    /// and are not part of this module's source (a nested Go module). They are
    /// left out of this module's source identity, matching change attribution,
    /// which already assigns them to their own module. Adapter-set only where
    /// the build tool itself excludes them; a module whose tasks read its
    /// whole tree keeps this empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_subtrees: Vec<RepoPath>,
    /// Whether the module's manifest forbids registry publication (an npm
    /// `"private": true` package). Adapter-set during discovery; release
    /// excludes such a module unless config sets `exclude = false`, which makes
    /// it tag-only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unpublishable: bool,
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
            capabilities: BTreeSet::new(),
            cache_inputs_complete: true,
            external_roots: Vec::new(),
            excluded_subtrees: Vec::new(),
            unpublishable: false,
        }
    }

    /// The registry package name (`@acme/core`), or the module name when the
    /// module declares none.
    #[must_use]
    pub fn package_name(&self) -> &str {
        self.package.as_deref().unwrap_or(&self.id.name)
    }

    /// Whether this module provides every capability in `required`.
    #[must_use]
    pub fn provides<'a>(&self, required: impl IntoIterator<Item = &'a Capability>) -> bool {
        required
            .into_iter()
            .all(|capability| self.capabilities.contains(capability))
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
    use crate::capability::Capability;
    use crate::identity::{EcosystemId, ModuleRef, RepoPath};

    #[test]
    fn serde_round_trip_omits_empty_options() {
        let module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            RepoPath::new("core/errors").unwrap(),
        );
        let json = serde_json::to_string(&module).unwrap();
        assert!(!json.contains("package"));
        assert!(!json.contains("capabilities"));
        assert!(!json.contains("excluded_subtrees"));
        let back: Module = serde_json::from_str(&json).unwrap();
        assert_eq!(module, back);
        assert!(back.cache_inputs_complete);
        assert!(json.contains("cache_inputs_complete"));
    }

    #[test]
    fn capabilities_round_trip_and_gate_requirements() {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("js").unwrap(), "std").unwrap(),
            RepoPath::new("packages/std").unwrap(),
        );
        let test = Capability::new("script:test").unwrap();
        let build = Capability::new("script:build").unwrap();
        module.capabilities.insert(test.clone());
        let json = serde_json::to_string(&module).unwrap();
        assert!(
            json.contains("\"capabilities\":[\"script:test\"]"),
            "{json}"
        );
        let back: Module = serde_json::from_str(&json).unwrap();
        assert_eq!(module, back);
        assert!(back.provides([]));
        assert!(back.provides([&test]));
        assert!(!back.provides([&test, &build]));
    }

    #[test]
    fn incomplete_cache_inputs_round_trip() {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("go").unwrap(), "app").unwrap(),
            RepoPath::new("app").unwrap(),
        );
        module.cache_inputs_complete = false;
        let json = serde_json::to_string(&module).unwrap();
        let back: Module = serde_json::from_str(&json).unwrap();
        assert!(!back.cache_inputs_complete);
        assert_eq!(module, back);
    }

    #[test]
    fn discovery_without_cache_input_completeness_is_not_trusted() {
        let module = Module::new(
            ModuleRef::new(EcosystemId::new("go").unwrap(), "app").unwrap(),
            RepoPath::new("app").unwrap(),
        );
        let mut json = serde_json::to_value(module).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("cache_inputs_complete");
        let back: Module = serde_json::from_value(json).unwrap();
        assert!(!back.cache_inputs_complete);
    }

    #[test]
    fn unpublishable_round_trips_and_is_omitted_when_false() {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("js").unwrap(), "tools").unwrap(),
            RepoPath::new("tools").unwrap(),
        );
        let json = serde_json::to_string(&module).unwrap();
        assert!(!json.contains("unpublishable"), "{json}");
        module.unpublishable = true;
        let json = serde_json::to_string(&module).unwrap();
        let back: Module = serde_json::from_str(&json).unwrap();
        assert!(back.unpublishable);
        assert_eq!(module, back);
    }

    #[test]
    fn package_name_prefers_the_registry_name() {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("js").unwrap(), "core").unwrap(),
            RepoPath::new("packages/core").unwrap(),
        );
        assert_eq!(module.package_name(), "core");
        module.package = Some("@acme/core".to_string());
        assert_eq!(module.package_name(), "@acme/core");
    }
}
