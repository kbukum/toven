//! Resolved release settings: fold the ecosystem-default release config and a
//! per-module override into the typed value the release engine consumes.
//!
//! Precedence (documented, per-run bump argv layered on top):
//! `[modules.<name>.release]` > `[ecosystems.<id>].release` > adapter default.

use rskit_errors::AppResult;
use toven_ports::{
    BumpLevel, ChangelogConfig, DependentVersion, HooksConfig, PrereleaseConfig, ReleaseConfig,
    SignConfig, merge_release,
};

use super::{ReleaseStrategyName, strategy};

/// Default release tag template when none is configured (`v1.2.3`).
const DEFAULT_TAG_FORMAT: &str = "v{version}";
/// Default git remote when none is configured.
const DEFAULT_REMOTE: &str = "origin";

/// The fully-resolved, defaults-applied release settings for one module.
///
/// Produced by folding the ecosystem `[ecosystems.<id>].release` default with an
/// optional `[modules.<name>.release]` override, then applying the built-in
/// defaults for anything still unset. The per-run bump argv layers over this
/// resolved value.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct ResolvedReleaseSettings {
    /// Resolved engine-owned bump strategy.
    pub strategy: ReleaseStrategyName,
    /// Default bump level applied to a changed module.
    pub level: BumpLevel,
    /// How a dependency-floor bump cascades into dependents.
    pub dependent_version: DependentVersion,
    /// Prerelease channels and the branch→channel mapping.
    pub prerelease: PrereleaseConfig,
    /// Release tag name template.
    pub tag_format: String,
    /// Annotated-tag message template; `None` = a lightweight tag.
    pub tag_message: Option<String>,
    /// Release commit message template; `None` = adapter default.
    pub commit_message: Option<String>,
    /// Changelog generation settings.
    pub changelog: ChangelogConfig,
    /// Whether the release commit/tags are pushed.
    pub push: bool,
    /// Git remote pushed to.
    pub remote: String,
    /// Allowed release branches; empty = any branch.
    pub branches: Vec<String>,
    /// Target registry identifier; `None` = not publishable.
    pub registry: Option<String>,
    /// Whether registry lookups are skipped (idempotency anchored on tags only).
    pub offline: bool,
    /// Environment-variable name holding the registry token (never the secret).
    pub token_env: Option<String>,
    /// Artifact-signing settings.
    pub sign: SignConfig,
    /// Recognized checks composing `release readiness`.
    pub readiness: Vec<String>,
    /// Optional pre/post release hooks.
    pub hooks: HooksConfig,
}

impl ResolvedReleaseSettings {
    /// Resolve settings for a module from its ecosystem default and optional
    /// per-module override, applying built-in defaults for anything unset.
    ///
    /// # Errors
    /// Propagates an unknown/malformed release strategy from the merged config.
    pub fn resolve(ecosystem: &ReleaseConfig, module: Option<&ReleaseConfig>) -> AppResult<Self> {
        let merged =
            module.map_or_else(|| ecosystem.clone(), |over| merge_release(ecosystem, over));
        Self::from_merged(&merged)
    }

    /// Apply defaults and resolve the strategy over an already-merged config.
    fn from_merged(config: &ReleaseConfig) -> AppResult<Self> {
        Ok(Self {
            strategy: strategy::resolve(config.strategy.as_deref())?,
            level: config.level.unwrap_or(BumpLevel::Auto),
            dependent_version: config.dependent_version.unwrap_or(DependentVersion::Bump),
            prerelease: config.prerelease.clone(),
            tag_format: config
                .tag_format
                .clone()
                .unwrap_or_else(|| DEFAULT_TAG_FORMAT.to_string()),
            tag_message: config.tag_message.clone(),
            commit_message: config.commit_message.clone(),
            changelog: config.changelog.clone(),
            push: config.push.unwrap_or(true),
            remote: config
                .remote
                .clone()
                .unwrap_or_else(|| DEFAULT_REMOTE.to_string()),
            branches: config.branches.clone(),
            registry: config.registry.clone(),
            offline: config.offline.unwrap_or(false),
            token_env: config.token_env.clone(),
            sign: config.sign.clone(),
            readiness: config.readiness.clone(),
            hooks: config.hooks.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ResolvedReleaseSettings;
    use crate::release::ReleaseStrategyName;
    use toven_ports::{BumpLevel, ReleaseConfig};

    #[test]
    fn empty_config_yields_documented_defaults() {
        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert_eq!(resolved.strategy, ReleaseStrategyName::SemverCascade);
        assert_eq!(resolved.level, BumpLevel::Auto);
        assert_eq!(resolved.tag_format, "v{version}");
        assert!(resolved.push);
        assert_eq!(resolved.remote, "origin");
        assert!(!resolved.offline);
    }

    #[test]
    fn ecosystem_config_overrides_defaults() {
        let ecosystem = ReleaseConfig {
            level: Some(BumpLevel::Minor),
            registry: Some("crates-io".into()),
            offline: Some(true),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert_eq!(resolved.level, BumpLevel::Minor);
        assert_eq!(resolved.registry.as_deref(), Some("crates-io"));
        assert!(resolved.offline);
    }

    #[test]
    fn module_override_wins_over_ecosystem() {
        let ecosystem = ReleaseConfig {
            level: Some(BumpLevel::Minor),
            registry: Some("crates-io".into()),
            ..ReleaseConfig::default()
        };
        let module = ReleaseConfig {
            level: Some(BumpLevel::Major),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, Some(&module)).unwrap();
        // per-module override wins on level, inherits ecosystem registry:
        assert_eq!(resolved.level, BumpLevel::Major);
        assert_eq!(resolved.registry.as_deref(), Some("crates-io"));
    }

    #[test]
    fn unknown_strategy_is_a_typed_error() {
        let ecosystem = ReleaseConfig {
            strategy: Some("nonsense".into()),
            ..ReleaseConfig::default()
        };
        assert!(ResolvedReleaseSettings::resolve(&ecosystem, None).is_err());
    }
}
