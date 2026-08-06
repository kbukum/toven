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

    /// The forced level for `module`, if any.
    pub(crate) fn module_level(&self, module: &ModuleRef) -> Option<BumpLevel> {
        self.module_levels.get(module).copied()
    }

    /// The pinned version for `module`, if any.
    pub(crate) fn set_version(&self, module: &ModuleRef) -> Option<&Version> {
        self.set_versions.get(module)
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
}
