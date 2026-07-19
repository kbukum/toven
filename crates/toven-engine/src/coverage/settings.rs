//! Resolved coverage settings: fold the ecosystem-default coverage config, an
//! elevated profile, and a per-module override into the typed value the gate
//! consumes.
//!
//! Precedence (documented): `[modules.<name>.coverage]` > `profiles.<name>` >
//! `[ecosystems.<id>].coverage` > adapter default. The structural twin of
//! [`ResolvedReleaseSettings`](crate::release::ResolvedReleaseSettings).

use toven_ports::{CoverageConfig, CoverageThresholds, Enforcement, merge_coverage};

/// The fully-resolved, defaults-applied coverage settings for one module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedCoverageSettings {
    /// The per-dimension floors this module is gated against.
    pub thresholds: CoverageThresholds,
    /// How a below-threshold verdict is enforced (`block` unless overridden).
    pub enforcement: Enforcement,
    /// Whether the module is measured but never gated (in the ecosystem
    /// `exclude` list).
    pub excluded: bool,
}

/// Per-run threshold overrides sourced from argv, layered over the resolved
/// config so argv wins and config is only the default.
///
/// Each field is `None` unless the user set the matching flag; a set field
/// replaces the corresponding resolved floor (or enforcement) for every gated
/// module. This is the coverage twin of release's per-run bump overrides: the
/// config is the durable default, the flag is the one-off override.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CoverageOverrides {
    /// Override the absolute line floor.
    pub line: Option<f64>,
    /// Override the function floor.
    pub function: Option<f64>,
    /// Override the region floor.
    pub region: Option<f64>,
    /// Override the changed-lines floor.
    pub changed_line: Option<f64>,
    /// Override the enforcement mode.
    pub enforcement: Option<Enforcement>,
}

impl CoverageOverrides {
    /// Whether no override is set (so resolution can skip the override layer).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.line.is_none()
            && self.function.is_none()
            && self.region.is_none()
            && self.changed_line.is_none()
            && self.enforcement.is_none()
    }
}

impl ResolvedCoverageSettings {
    /// Resolve settings for a module named `module` (its ecosystem-local name)
    /// from its ecosystem coverage default and optional per-module override.
    ///
    /// The ecosystem default is layered first, then the matching profile (if
    /// the module is named by one), then the per-module override — each folded
    /// with [`merge_coverage`] so a later layer only replaces the fields it
    /// sets, which realizes the documented precedence
    /// `[modules.<name>.coverage]` > `profiles.<name>` >
    /// `[ecosystems.<id>].coverage`. Exclusion is an ecosystem-level decision.
    #[must_use]
    pub fn resolve(
        ecosystem: &CoverageConfig,
        module: &str,
        over: Option<&CoverageConfig>,
    ) -> Self {
        let mut merged = ecosystem.clone();
        if let Some(profile) = ecosystem.profile_for(module) {
            merged = merge_coverage(&merged, &profile_as_config(profile));
        }
        if let Some(over) = over {
            merged = merge_coverage(&merged, over);
        }

        Self {
            thresholds: merged.thresholds(),
            enforcement: merged.enforcement.unwrap_or_default(),
            excluded: ecosystem.is_excluded(module),
        }
    }

    /// Layer the per-run argv overrides over the resolved config.
    ///
    /// Each set override replaces the corresponding resolved floor (or the
    /// enforcement mode), so argv wins over config; an empty override leaves
    /// the resolved settings untouched.
    #[must_use]
    pub const fn with_overrides(mut self, overrides: &CoverageOverrides) -> Self {
        if let Some(line) = overrides.line {
            self.thresholds.line = Some(line);
        }
        if let Some(function) = overrides.function {
            self.thresholds.function = Some(function);
        }
        if let Some(region) = overrides.region {
            self.thresholds.region = Some(region);
        }
        if let Some(changed_line) = overrides.changed_line {
            self.thresholds.changed_line = Some(changed_line);
        }
        if let Some(enforcement) = overrides.enforcement {
            self.enforcement = enforcement;
        }
        self
    }
}

/// Lift a profile's thresholds/enforcement into a `CoverageConfig` layer so it
/// folds through [`merge_coverage`] between the ecosystem default and a
/// per-module override.
fn profile_as_config(profile: &toven_ports::CoverageProfile) -> CoverageConfig {
    CoverageConfig {
        line: profile.line,
        function: profile.function,
        region: profile.region,
        changed_line: profile.changed_line,
        enforcement: profile.enforcement,
        ..CoverageConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::ResolvedCoverageSettings;
    use std::collections::BTreeMap;
    use toven_ports::{CoverageConfig, CoverageProfile, Enforcement};

    fn ecosystem() -> CoverageConfig {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "security".to_string(),
            CoverageProfile {
                line: Some(95.0),
                modules: vec!["toven-auth".into()],
                ..CoverageProfile::default()
            },
        );
        CoverageConfig {
            line: Some(90.0),
            function: Some(85.0),
            enforcement: Some(Enforcement::Block),
            exclude: vec!["toven-suite".into()],
            profiles,
            ..CoverageConfig::default()
        }
    }

    #[test]
    fn ecosystem_default_applies_to_a_plain_module() {
        let resolved = ResolvedCoverageSettings::resolve(&ecosystem(), "toven-process", None);
        assert_eq!(resolved.thresholds.line, Some(90.0));
        assert_eq!(resolved.thresholds.function, Some(85.0));
        assert_eq!(resolved.enforcement, Enforcement::Block);
        assert!(!resolved.excluded);
    }

    #[test]
    fn profile_beats_ecosystem_default() {
        let resolved = ResolvedCoverageSettings::resolve(&ecosystem(), "toven-auth", None);
        assert_eq!(resolved.thresholds.line, Some(95.0));
        // the profile only elevates `line`; `function` inherits the ecosystem.
        assert_eq!(resolved.thresholds.function, Some(85.0));
    }

    #[test]
    fn module_override_beats_profile_and_ecosystem() {
        let over = CoverageConfig {
            line: Some(80.0),
            enforcement: Some(Enforcement::Advisory),
            ..CoverageConfig::default()
        };
        let resolved = ResolvedCoverageSettings::resolve(&ecosystem(), "toven-auth", Some(&over));
        assert_eq!(resolved.thresholds.line, Some(80.0));
        assert_eq!(resolved.enforcement, Enforcement::Advisory);
    }

    #[test]
    fn excluded_module_is_flagged() {
        let resolved = ResolvedCoverageSettings::resolve(&ecosystem(), "toven-suite", None);
        assert!(resolved.excluded);
    }

    #[test]
    fn enforcement_defaults_to_block_when_unset() {
        let resolved = ResolvedCoverageSettings::resolve(&CoverageConfig::default(), "x", None);
        assert_eq!(resolved.enforcement, Enforcement::Block);
    }

    #[test]
    fn argv_overrides_beat_resolved_config() {
        use super::CoverageOverrides;
        let resolved = ResolvedCoverageSettings::resolve(&ecosystem(), "toven-process", None);
        assert_eq!(resolved.thresholds.line, Some(90.0));
        let overridden = resolved.with_overrides(&CoverageOverrides {
            line: Some(70.0),
            enforcement: Some(Enforcement::Advisory),
            ..CoverageOverrides::default()
        });
        assert_eq!(overridden.thresholds.line, Some(70.0));
        assert_eq!(overridden.enforcement, Enforcement::Advisory);
        // an unset override leaves the resolved value in place.
        assert_eq!(overridden.thresholds.function, Some(85.0));
    }

    #[test]
    fn empty_override_leaves_settings_untouched() {
        use super::CoverageOverrides;
        let resolved = ResolvedCoverageSettings::resolve(&ecosystem(), "toven-process", None);
        assert!(CoverageOverrides::default().is_empty());
        assert_eq!(
            resolved.with_overrides(&CoverageOverrides::default()),
            resolved
        );
    }
}
