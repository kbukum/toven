//! Resolved release settings: fold the ecosystem-default release config and a
//! per-module override into the typed value the release engine consumes.
//!
//! Precedence (documented, per-run bump argv layered on top):
//! `[modules.<name>.release]` > `[ecosystems.<id>].release` > adapter default.

use rskit_errors::AppResult;
use toven_ports::{
    BumpLevel, ChangelogConfig, DependentVersion, HooksConfig, HostConfig, PrereleaseConfig,
    PublicationPolicy, ReleaseConfig, SignConfig, merge_release,
};

use super::{BumpPolicy, strategy};

/// Default git remote when none is configured.
const DEFAULT_REMOTE: &str = "origin";
/// Default workspace-relative changelog path when none is configured.
const DEFAULT_CHANGELOG_PATH: &str = "CHANGELOG.md";

/// Fully-resolved hosted-release settings for one module.
///
/// Folded from the `[…release].host` block. With no configured `forge`, the
/// release pipeline stops after tag/registry publish and no hosted Release is
/// cut. `prerelease` stays `None` when unset so the engine derives it from the
/// released version's prerelease channel.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ResolvedHostSettings {
    /// Forge that hosts the Release; `None` = no hosted Release.
    pub forge: Option<String>,
    /// Whether the Release is cut as a draft.
    pub draft: bool,
    /// Explicit prerelease flag; `None` = derive from the version channel.
    pub prerelease: Option<bool>,
    /// Explicit release-note body; `None` = source from the changelog.
    pub notes: Option<String>,
    /// Project-relative artifact paths uploaded to the Release.
    pub assets: Vec<String>,
}

impl ResolvedHostSettings {
    /// Fold a configured `[…release].host` block into resolved settings.
    fn from_config(config: Option<&HostConfig>) -> Self {
        let Some(config) = config else {
            return Self::default();
        };
        Self {
            forge: config.forge.clone(),
            draft: config.draft.unwrap_or(false),
            prerelease: config.prerelease,
            notes: config.notes.clone(),
            assets: config.assets.clone().unwrap_or_default(),
        }
    }
}

/// The fully-resolved, defaults-applied release settings for one module.
///
/// Produced by folding the ecosystem `[ecosystems.<id>].release` default with
/// an optional `[modules.<name>.release]` override, then applying the built-in
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
    /// Typed publication policy resolved from `registry`/`publish`/`exclude`:
    /// registry publication, tag-only, or excluded from the release.
    pub publication: PublicationPolicy,
    /// Whether registry lookups are skipped (idempotency anchored on tags
    /// only).
    pub offline: bool,
    /// Environment-variable name holding the registry token (never the secret).
    pub token_env: Option<String>,
    /// Artifact-signing settings.
    pub sign: SignConfig,
    /// Recognized checks composing `release readiness`.
    pub readiness: Vec<String>,
    /// Optional pre/post release hooks.
    pub hooks: HooksConfig,
    /// Hosted forge Release settings.
    pub host: ResolvedHostSettings,
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

    /// Apply defaults and resolve the bump policy over an already-merged
    /// config.
    ///
    /// The `registry`/`publish`/`exclude` fields are **not** re-validated here:
    /// each raw block (ecosystem default and per-module override) is checked for
    /// same-block contradictions at load, and [`PublicationPolicy::resolve`] is
    /// total, so a more-specific per-module override may legitimately narrow the
    /// inherited publication (an inherited registry with `exclude = true`
    /// resolves to `Excluded`, and with `publish = false` to `TagOnly`) without
    /// tripping a false merged-level contradiction.
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
            publication: PublicationPolicy::resolve(
                config.registry.as_deref(),
                config.publish,
                config.exclude.unwrap_or(false),
            ),
            offline: config.offline.unwrap_or(false),
            token_env: config.token_env.clone(),
            sign: config.sign.clone().unwrap_or_default(),
            readiness: config.readiness.clone().unwrap_or_default(),
            hooks: config.hooks.clone().unwrap_or_default(),
            host: ResolvedHostSettings::from_config(config.host.as_ref()),
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
    use toven_ports::{BumpLevel, PublicationPolicy, ReleaseConfig};

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
        assert_eq!(
            resolved.publication,
            PublicationPolicy::Registry {
                registry: "crates-io".into()
            }
        );
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
        assert_eq!(
            resolved.publication,
            PublicationPolicy::Registry {
                registry: "crates-io".into()
            }
        );
    }

    #[test]
    fn module_override_excludes_under_a_registry_ecosystem() {
        // A registry ecosystem (crates.io) with a per-module `exclude = true`
        // override must resolve to `Excluded` — the inherited registry does not
        // make the combination contradictory, because exclusion is a
        // deliberate, more-specific narrowing (e.g. an example or fuzz crate).
        let ecosystem = ReleaseConfig {
            registry: Some("crates-io".into()),
            ..ReleaseConfig::default()
        };
        let module = ReleaseConfig {
            exclude: Some(true),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, Some(&module)).unwrap();
        assert_eq!(resolved.publication, PublicationPolicy::Excluded);
        assert!(!resolved.publication.releases());
    }

    #[test]
    fn module_override_makes_one_module_tag_only_under_a_registry_ecosystem() {
        // A per-module `publish = false` narrows a single crate to a tag-only
        // release while its siblings keep publishing to the inherited registry.
        let ecosystem = ReleaseConfig {
            registry: Some("crates-io".into()),
            ..ReleaseConfig::default()
        };
        let module = ReleaseConfig {
            publish: Some(false),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, Some(&module)).unwrap();
        assert_eq!(resolved.publication, PublicationPolicy::TagOnly);
    }

    #[test]
    fn host_settings_default_to_no_hosted_release() {
        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert_eq!(resolved.host.forge, None);
        assert!(!resolved.host.draft);
        assert_eq!(resolved.host.prerelease, None);
        assert!(resolved.host.assets.is_empty());
    }

    #[test]
    fn host_config_resolves_into_settings() {
        use toven_ports::HostConfig;

        let ecosystem = ReleaseConfig {
            host: Some(HostConfig {
                forge: Some("github".into()),
                draft: Some(true),
                assets: Some(vec!["target/toven/release/core.cdx.json".into()]),
                ..HostConfig::default()
            }),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert_eq!(resolved.host.forge.as_deref(), Some("github"));
        assert!(resolved.host.draft);
        assert_eq!(resolved.host.prerelease, None);
        assert_eq!(resolved.host.assets.len(), 1);
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
