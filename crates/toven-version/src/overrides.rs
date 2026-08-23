//! Per-run bump argv: the explicit overrides layered over resolved config.
//!
//! The overrides carry the mutating release actions' bump argv
//! (`--patch`/`--minor`/`--major`/`--set-version`/`--pre`/`--base`/`--offline`)
//! as typed, validated data. They layer over the resolved config with the
//! documented precedence (**argv > `[modules.<name>.release]` >
//! `[ecosystems.<id>].release` > adapter default**); the config side never
//! rewrites user argv, and every override is validated at the CLI boundary
//! before it reaches the bump planner.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use toven_model::ModuleRef;
use toven_ports::BumpLevel;

/// The explicit, validated per-run bump overrides.
///
/// Built at the CLI boundary from the parsed argv; a conflicting combination (a
/// module in two level flags, or in both a level flag and `--set-version`) is
/// rejected as a typed error at construction time.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct BumpOverrides {
    module_levels: BTreeMap<ModuleRef, BumpLevel>,
    set_versions: BTreeMap<ModuleRef, Version>,
    workspace_level: Option<BumpLevel>,
    workspace_set_version: Option<Version>,
    prerelease: Option<String>,
    base: Option<String>,
    offline: bool,
}

impl BumpOverrides {
    /// An empty override set (config decides everything).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Force `module` to bump at `level`.
    ///
    /// # Errors
    /// Rejects `BumpLevel::Auto` (a per-run override is always an explicit
    /// `patch`/`minor`/`major`), a module already forced to a different level,
    /// or one pinned by `--set-version`.
    pub fn with_module_level(mut self, module: ModuleRef, level: BumpLevel) -> AppResult<Self> {
        if level == BumpLevel::Auto {
            return Err(AppError::invalid_input(
                "release.bump",
                format!("bump override for module '{module}' must be patch, minor, or major"),
            ));
        }
        if self.set_versions.contains_key(&module) {
            return Err(conflict(&module));
        }
        if let Some(existing) = self.module_levels.get(&module)
            && *existing != level
        {
            return Err(conflict(&module));
        }
        self.module_levels.insert(module, level);
        Ok(self)
    }

    /// Pin `module` to an explicit target `version`.
    ///
    /// # Errors
    /// Rejects a module already forced to a level or pinned to a different
    /// version.
    pub fn with_set_version(mut self, module: ModuleRef, version: Version) -> AppResult<Self> {
        if self.module_levels.contains_key(&module) {
            return Err(conflict(&module));
        }
        if let Some(existing) = self.set_versions.get(&module)
            && *existing != version
        {
            return Err(conflict(&module));
        }
        self.set_versions.insert(module, version);
        Ok(self)
    }

    /// Force every in-scope module to the same explicit target `version`
    /// (lock-step / ecosystem-wide), unless a per-module override names it.
    ///
    /// The single-argv lock-step target: every release-scope module — root and
    /// submodules, changed or not, tagged or brand-new — is put on `version`,
    /// so a lock-step-tag-all repository cuts one version across the whole set
    /// without a per-module flag each. A per-module `--set-version`/level still
    /// wins for the module it names (argv-precedence within the same run).
    ///
    /// # Errors
    /// Rejects a run that already carries a workspace-wide level (a workspace
    /// target and a workspace level are mutually exclusive).
    pub fn with_workspace_set_version(mut self, version: Version) -> AppResult<Self> {
        if self.workspace_level.is_some() {
            return Err(workspace_conflict());
        }
        self.workspace_set_version = Some(version);
        Ok(self)
    }

    /// Force every in-scope module to bump at the same `level` (lock-step /
    /// ecosystem-wide), unless a per-module override names it.
    ///
    /// # Errors
    /// Rejects `BumpLevel::Auto` (a workspace level is always an explicit
    /// `patch`/`minor`/`major`) or a run that already carries a workspace-wide
    /// target.
    pub fn with_workspace_level(mut self, level: BumpLevel) -> AppResult<Self> {
        if level == BumpLevel::Auto {
            return Err(AppError::invalid_input(
                "release.bump",
                "workspace bump level must be patch, minor, or major",
            ));
        }
        if self.workspace_set_version.is_some() {
            return Err(workspace_conflict());
        }
        self.workspace_level = Some(level);
        Ok(self)
    }

    /// Cut a prerelease on `channel`.
    #[must_use]
    pub fn with_prerelease(mut self, channel: impl Into<String>) -> Self {
        self.prerelease = Some(channel.into());
        self
    }

    /// Set the git ref to diff against for change detection.
    #[must_use]
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = Some(base.into());
        self
    }

    /// Skip registry lookups and anchor idempotency on the release tag only.
    #[must_use]
    pub const fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// The prerelease channel requested for this run, if any.
    #[must_use]
    pub fn prerelease(&self) -> Option<&str> {
        self.prerelease.as_deref()
    }

    /// The base ref requested for change detection, if any.
    #[must_use]
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// Whether registry lookups are skipped for this run.
    #[must_use]
    pub const fn offline(&self) -> bool {
        self.offline
    }

    /// The forced level for `module`, if any (per-module, else workspace-wide).
    pub(crate) fn module_level(&self, module: &ModuleRef) -> Option<BumpLevel> {
        if let Some(level) = self.module_levels.get(module).copied() {
            return Some(level);
        }
        // A per-module target pins the module and takes the set-version path, so
        // the workspace level must not also apply to it.
        if self.set_versions.contains_key(module) {
            return None;
        }
        self.workspace_level
    }

    /// The pinned version for `module`, if any (per-module, else workspace-wide).
    pub(crate) fn set_version(&self, module: &ModuleRef) -> Option<&Version> {
        if let Some(version) = self.set_versions.get(module) {
            return Some(version);
        }
        // A per-module level pins the module and takes the level path, so the
        // workspace target must not also apply to it.
        if self.module_levels.contains_key(module) {
            return None;
        }
        self.workspace_set_version.as_ref()
    }

    /// Whether an explicit override (per-module or workspace-wide) forces
    /// `module` into the release, even when it is otherwise unchanged.
    ///
    /// A `--set-version`/level override is an explicit instruction to release
    /// the named module (or, workspace-wide, every module) at a version, so it
    /// must be included even if its own tracked files did not change since the
    /// baseline — the root/hosted module of a lock-step set is the canonical
    /// case.
    pub(crate) fn forces(&self, module: &ModuleRef) -> bool {
        self.set_version(module).is_some() || self.module_level(module).is_some()
    }

    /// Validate that every module named in an override is in the release scope.
    ///
    /// # Errors
    /// Rejects an override naming a module absent from the release scope
    /// (either unknown or unchanged), so a typo can never silently no-op.
    pub(crate) fn validate_known(&self, known: &BTreeSet<ModuleRef>) -> AppResult<()> {
        for module in self.module_levels.keys().chain(self.set_versions.keys()) {
            if !known.contains(module) {
                return Err(AppError::invalid_input(
                    "release.bump",
                    format!(
                        "module '{module}' named in a bump override is not in the release scope"
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// The typed error for a module forced to conflicting bump inputs.
fn conflict(module: &ModuleRef) -> AppError {
    AppError::invalid_input(
        "release.bump",
        format!("conflicting bump overrides for module '{module}'"),
    )
}

/// The typed error for a run that mixes a workspace-wide target and level.
fn workspace_conflict() -> AppError {
    AppError::invalid_input(
        "release.bump",
        "a workspace-wide target version and a workspace-wide bump level cannot be combined; \
         choose one",
    )
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleRef};
    use toven_ports::BumpLevel;

    use super::BumpOverrides;

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    #[test]
    fn level_and_set_version_on_same_module_conflict() {
        let overrides = BumpOverrides::new()
            .with_module_level(mref("core"), BumpLevel::Minor)
            .unwrap();
        assert!(
            overrides
                .with_set_version(mref("core"), Version::new(1, 0, 0))
                .is_err()
        );
    }

    #[test]
    fn conflicting_levels_on_same_module_are_rejected() {
        let overrides = BumpOverrides::new()
            .with_module_level(mref("core"), BumpLevel::Minor)
            .unwrap();
        assert!(
            overrides
                .with_module_level(mref("core"), BumpLevel::Major)
                .is_err()
        );
    }

    #[test]
    fn repeated_identical_level_is_idempotent() {
        let overrides = BumpOverrides::new()
            .with_module_level(mref("core"), BumpLevel::Minor)
            .unwrap()
            .with_module_level(mref("core"), BumpLevel::Minor)
            .unwrap();
        assert_eq!(
            overrides.module_level(&mref("core")),
            Some(BumpLevel::Minor)
        );
    }

    #[test]
    fn unknown_module_override_is_rejected() {
        let overrides = BumpOverrides::new()
            .with_module_level(mref("missing"), BumpLevel::Major)
            .unwrap();
        let known = std::iter::once(mref("core")).collect();
        assert!(overrides.validate_known(&known).is_err());
    }

    #[test]
    fn an_auto_level_override_is_rejected() {
        let result = BumpOverrides::new().with_module_level(mref("core"), BumpLevel::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn a_workspace_target_and_workspace_level_cannot_combine() {
        let with_target = BumpOverrides::new()
            .with_workspace_set_version(Version::new(0, 3, 0))
            .unwrap();
        assert!(with_target.with_workspace_level(BumpLevel::Minor).is_err());

        let with_level = BumpOverrides::new()
            .with_workspace_level(BumpLevel::Minor)
            .unwrap();
        assert!(
            with_level
                .with_workspace_set_version(Version::new(0, 3, 0))
                .is_err()
        );
    }

    #[test]
    fn a_workspace_target_applies_to_every_module() {
        let overrides = BumpOverrides::new()
            .with_workspace_set_version(Version::new(0, 3, 0))
            .unwrap();
        assert_eq!(
            overrides.set_version(&mref("root")),
            Some(&Version::new(0, 3, 0))
        );
        assert_eq!(
            overrides.set_version(&mref("submodule")),
            Some(&Version::new(0, 3, 0))
        );
        assert!(overrides.forces(&mref("anything")));
    }

    #[test]
    fn a_per_module_target_wins_over_the_workspace_target() {
        let overrides = BumpOverrides::new()
            .with_workspace_set_version(Version::new(0, 3, 0))
            .unwrap()
            .with_set_version(mref("core"), Version::new(1, 0, 0))
            .unwrap();
        assert_eq!(
            overrides.set_version(&mref("core")),
            Some(&Version::new(1, 0, 0)),
            "the per-module pin wins for the module it names"
        );
        assert_eq!(
            overrides.set_version(&mref("other")),
            Some(&Version::new(0, 3, 0)),
            "every other module still takes the workspace target"
        );
    }

    #[test]
    fn a_per_module_level_wins_over_the_workspace_target_for_its_module() {
        let overrides = BumpOverrides::new()
            .with_workspace_set_version(Version::new(0, 3, 0))
            .unwrap()
            .with_module_level(mref("core"), BumpLevel::Major)
            .unwrap();
        // The named module takes its level path, not the workspace target.
        assert_eq!(overrides.set_version(&mref("core")), None);
        assert_eq!(
            overrides.module_level(&mref("core")),
            Some(BumpLevel::Major)
        );
        // Other modules still take the workspace target.
        assert_eq!(
            overrides.set_version(&mref("other")),
            Some(&Version::new(0, 3, 0))
        );
        assert_eq!(overrides.module_level(&mref("other")), None);
    }

    #[test]
    fn a_workspace_level_applies_to_every_module() {
        let overrides = BumpOverrides::new()
            .with_workspace_level(BumpLevel::Minor)
            .unwrap();
        assert_eq!(
            overrides.module_level(&mref("root")),
            Some(BumpLevel::Minor)
        );
        assert_eq!(
            overrides.module_level(&mref("leaf")),
            Some(BumpLevel::Minor)
        );
        assert!(overrides.forces(&mref("leaf")));
    }

    #[test]
    fn an_auto_workspace_level_is_rejected() {
        assert!(
            BumpOverrides::new()
                .with_workspace_level(BumpLevel::Auto)
                .is_err()
        );
    }
}
