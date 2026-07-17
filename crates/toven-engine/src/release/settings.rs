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

use super::{BumpPolicy, strategy};

/// Default git remote when none is configured.
const DEFAULT_REMOTE: &str = "origin";
/// Default workspace-relative changelog path when none is configured.
const DEFAULT_CHANGELOG_PATH: &str = "CHANGELOG.md";

/// The fully-resolved, defaults-applied release settings for one module.
///
/// Produced by folding the ecosystem `[ecosystems.<id>].release` default with an
/// optional `[modules.<name>.release]` override, then applying the built-in
/// defaults for anything still unset. The per-run bump argv layers over this
/// resolved value.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct ResolvedReleaseSettings {
    /// Resolved engine-owned bump policy.
    pub policy: BumpPolicy,
    /// Default bump level applied to a changed module.
    pub level: BumpLevel,
    /// How a dependency-floor bump cascades into dependents.
    pub dependent_version: DependentVersion,
    /// Prerelease channels and the branch→channel mapping.
    pub prerelease: PrereleaseConfig,
    /// Configured release tag name template; `None` = target default.
    pub tag_format: Option<String>,
    /// Annotated-tag message template; `None` = a lightweight tag.
    pub tag_message: Option<String>,
    /// Release commit message template; `None` = adapter default.
    pub commit_message: Option<String>,
    /// Changelog generation settings; `path` is defaulted to `CHANGELOG.md`.
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
    /// Propagates an unknown/malformed bump policy from the merged config's
    /// `strategy` field.
    pub fn resolve(ecosystem: &ReleaseConfig, module: Option<&ReleaseConfig>) -> AppResult<Self> {
        let merged =
            module.map_or_else(|| ecosystem.clone(), |over| merge_release(ecosystem, over));
        Self::from_merged(&merged)
    }

    /// Apply defaults and resolve the bump policy over an already-merged config.
    fn from_merged(config: &ReleaseConfig) -> AppResult<Self> {
        Ok(Self {
            policy: strategy::resolve(config.strategy.as_deref())?,
            level: config.level.unwrap_or(BumpLevel::Auto),
            dependent_version: config.dependent_version.unwrap_or(DependentVersion::Bump),
            prerelease: config.prerelease.clone().unwrap_or_default(),
            tag_format: config.tag_format.clone(),
            tag_message: config.tag_message.clone(),
            commit_message: config.commit_message.clone(),
            changelog: resolve_changelog(config.changelog.clone().unwrap_or_default()),
            push: config.push.unwrap_or(true),
            remote: config
                .remote
                .clone()
                .unwrap_or_else(|| DEFAULT_REMOTE.to_string()),
            branches: config.branches.clone().unwrap_or_default(),
            registry: config.registry.clone(),
            offline: config.offline.unwrap_or(false),
            token_env: config.token_env.clone(),
            sign: config.sign.clone().unwrap_or_default(),
            readiness: config.readiness.clone().unwrap_or_default(),
            hooks: config.hooks.clone().unwrap_or_default(),
        })
    }
}

/// Apply the built-in `CHANGELOG.md` default to an unset changelog path.
fn resolve_changelog(mut changelog: ChangelogConfig) -> ChangelogConfig {
    if changelog.path.is_none() {
        changelog.path = Some(DEFAULT_CHANGELOG_PATH.to_string());
    }
    changelog
}

#[cfg(test)]
mod tests {
    use super::ResolvedReleaseSettings;
    use crate::release::BumpPolicy;
    use toven_ports::{BumpLevel, ReleaseConfig};

    #[test]
    fn empty_config_yields_documented_defaults() {
        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert_eq!(resolved.policy, BumpPolicy::SemverCascade);
        assert_eq!(resolved.level, BumpLevel::Auto);
        assert_eq!(resolved.tag_format, None);
        assert!(resolved.push);
        assert_eq!(resolved.remote, "origin");
        assert!(!resolved.offline);
        assert_eq!(resolved.changelog.path.as_deref(), Some("CHANGELOG.md"));
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
