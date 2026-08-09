//! Resolved release settings: fold the ecosystem-default release config and a
//! per-module override into the typed value the release engine consumes.
//!
//! Precedence (documented, per-run bump argv layered on top):
//! `[modules.<name>.release]` > `[ecosystems.<id>].release` > adapter default.

use rskit_errors::AppResult;
use toven_model::{Entrypoint, ReleasePhase};
use toven_ports::{
    BumpLevel, ChangelogConfig, DependentVersion, HostConfig, ImageConfig, PhaseBacking,
    PhasesConfig, PrereleaseConfig, PublicationPolicy, ReleaseConfig, SignConfig, SignFormat,
    VersionReferenceConfig, Visibility, merge_release,
};

use crate::versioning::strategy;
use crate::{BumpPolicy, PushPolicy};

/// Parse a configured `sign_format` value onto the [`SignFormat`] backend enum.
///
/// Accepts the canonical git `gpg.format` values plus the common `gpg` alias for
/// `OpenPGP`; anything else is a typed configuration error.
fn parse_sign_format(value: &str) -> AppResult<SignFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openpgp" | "gpg" => Ok(SignFormat::OpenPgp),
        "ssh" => Ok(SignFormat::Ssh),
        "x509" => Ok(SignFormat::X509),
        other => Err(rskit_errors::AppError::invalid_input(
            "release.sign_format",
            format!("unknown sign_format '{other}'; use one of openpgp (or gpg), ssh, x509"),
        )),
    }
}

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
    /// Whether release tags are signed. Always implies an annotated tag
    /// (`tag_message` set) and an available signing key.
    pub sign_tags: bool,
    /// Signing backend for signed tags (`gpg.format`); `None` inherits git
    /// config. Only meaningful when `sign_tags` is set.
    pub sign_format: Option<SignFormat>,
    /// Signing key for signed tags (`user.signingkey`); `None` inherits git
    /// config. Carries the key *identifier* only. Only meaningful when
    /// `sign_tags` is set.
    pub signing_key: Option<String>,
    /// Release commit message template; `None` = adapter default.
    pub commit_message: Option<String>,
    /// Changelog generation settings; `path` is defaulted to `CHANGELOG.md`.
    pub changelog: ChangelogConfig,
    /// How the release commit/tags are pushed: branch and tags, tags only
    /// (tag-only mode for a protected branch), or not at all.
    pub push: PushPolicy,
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
    /// Exposure the release is cut with, enforced fail-closed at the
    /// registry-publish boundary; the tag push and hosted forge Release follow
    /// the remote repository's own exposure.
    pub visibility: Visibility,
    /// Artifact-signing settings.
    pub sign: SignConfig,
    /// Recognized checks composing `release readiness`.
    pub readiness: Vec<String>,
    /// Hosted forge Release settings.
    pub host: ResolvedHostSettings,
    /// Container-image phase settings; `None` = the module runs no image phase.
    pub image: Option<ImageConfig>,
    /// Per-phase backing map: how each release phase is satisfied (native, the
    /// default, or delegated to an external tool).
    pub phases: PhasesConfig,
    /// Who cuts the release: Toven (the default, owning the whole flow) or a
    /// maintainer (Toven runs against an existing human-created tag/Release).
    pub entrypoint: Entrypoint,
    /// Whether this module is the release train's umbrella aggregate — it
    /// contributes its members' notes to the shared hosted Release and does not
    /// publish to a registry unless it is itself a registry package.
    pub umbrella: bool,
    /// Version references: files whose embedded version tokens `release bump`
    /// keeps in lock-step with the authoritative post-bump versions.
    pub version_references: Vec<VersionReferenceConfig>,
    /// Bump `on-resolved` hooks: argv-first task references run mid-bump (after
    /// the version decision and native version-reference sync, before staging),
    /// each handed the authoritative post-bump version map.
    pub on_resolved: Vec<String>,
}

impl ResolvedReleaseSettings {
    /// The resolved backing for `phase` — [`PhaseBacking::Native`] when the
    /// phase has no configured entry.
    ///
    /// # Errors
    /// Propagates a configured-but-inconsistent phase entry (a delegated
    /// backing whose tool sub-block is missing or malformed).
    pub fn phase_backing(&self, phase: ReleasePhase) -> AppResult<PhaseBacking> {
        self.phases.backing(phase, "release.phases")
    }

    /// The delegated tool backing `phase`, if the phase delegates.
    ///
    /// Returns `None` for a native (or unconfigured) phase. The engine folds
    /// this into an argv-first [`DelegatedPhaseRequest`](toven_ports::DelegatedPhaseRequest)
    /// (via [`delegated_request`](crate::delegated_request)) when a
    /// phase resolves [`PhaseBacking::Delegated`].
    #[must_use]
    pub fn delegated_tool(&self, phase: ReleasePhase) -> Option<&toven_ports::DelegatedTool> {
        self.phases.delegated_tool(phase)
    }
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
        let sign_tags = config.sign_tags.unwrap_or(false);
        if sign_tags && config.tag_message.is_none() {
            return Err(rskit_errors::AppError::invalid_input(
                "release.sign_tags",
                "signed release tags are always annotated, so sign_tags = true requires a \
                 tag_message; set a tag_message template or disable sign_tags",
            ));
        }
        let sign_format = config
            .sign_format
            .as_deref()
            .map(parse_sign_format)
            .transpose()?;
        let signing_key = config.signing_key.clone();
        if !sign_tags && (sign_format.is_some() || signing_key.is_some()) {
            return Err(rskit_errors::AppError::invalid_input(
                "release.sign_tags",
                "sign_format and signing_key only apply to signed tags; set sign_tags = true or \
                 drop them",
            ));
        }
        Ok(Self {
            policy: strategy::resolve(config.strategy.as_deref())?,
            level: config.level.unwrap_or(BumpLevel::Auto),
            dependent_version: config.dependent_version.unwrap_or(DependentVersion::Bump),
            prerelease: config.prerelease.clone().unwrap_or_default(),
            tag_format: config.tag_format.clone(),
            tag_message: config.tag_message.clone(),
            sign_tags,
            sign_format,
            signing_key,
            commit_message: config.commit_message.clone(),
            changelog: resolve_changelog(config.changelog.clone().unwrap_or_default()),
            push: PushPolicy::resolve(
                config.push.unwrap_or(true),
                config.push_branch.unwrap_or(true),
            ),
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
            visibility: config.visibility.unwrap_or_default(),
            sign: config.sign.clone().unwrap_or_default(),
            readiness: config.readiness.clone().unwrap_or_default(),
            host: ResolvedHostSettings::from_config(config.host.as_ref()),
            image: config.image.clone(),
            phases: config.phases.clone().unwrap_or_default(),
            entrypoint: config.entrypoint.unwrap_or_default(),
            umbrella: config.umbrella.unwrap_or(false),
            version_references: config.version_references.clone().unwrap_or_default(),
            on_resolved: config.on_resolved.clone().unwrap_or_default(),
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
    use crate::{BumpPolicy, PushPolicy};
    use toven_ports::{BumpLevel, PublicationPolicy, ReleaseConfig};

    #[test]
    fn empty_config_yields_documented_defaults() {
        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert_eq!(resolved.policy, BumpPolicy::SemverCascade);
        assert_eq!(resolved.level, BumpLevel::Auto);
        assert_eq!(resolved.tag_format, None);
        assert_eq!(resolved.push, PushPolicy::BranchAndTags);
        assert_eq!(resolved.remote, "origin");
        assert!(!resolved.offline);
        assert!(!resolved.sign_tags);
        assert_eq!(resolved.changelog.path.as_deref(), Some("CHANGELOG.md"));
        assert_eq!(resolved.entrypoint, toven_model::Entrypoint::Toven);
        assert!(!resolved.umbrella);
    }

    #[test]
    fn entrypoint_and_umbrella_resolve_from_config() {
        let ecosystem = ReleaseConfig {
            entrypoint: Some(toven_model::Entrypoint::Maintainer),
            umbrella: Some(true),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert_eq!(resolved.entrypoint, toven_model::Entrypoint::Maintainer);
        assert!(resolved.umbrella);
    }

    #[test]
    fn sign_tags_resolves_with_an_annotated_message() {
        let ecosystem = ReleaseConfig {
            tag_message: Some("release {version}".into()),
            sign_tags: Some(true),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert!(resolved.sign_tags);
        assert_eq!(resolved.tag_message.as_deref(), Some("release {version}"));
    }

    #[test]
    fn sign_tags_without_a_tag_message_is_rejected() {
        let ecosystem = ReleaseConfig {
            sign_tags: Some(true),
            ..ReleaseConfig::default()
        };
        let error = ResolvedReleaseSettings::resolve(&ecosystem, None)
            .expect_err("signing requires an annotated tag");
        assert!(error.to_string().contains("sign_tags"), "{error}");
    }

    #[test]
    fn sign_tags_inherits_tag_message_from_the_ecosystem_default() {
        // The signing toggle and the annotation message may come from different
        // blocks: a module opts into signing while inheriting the ecosystem's
        // tag_message, and resolution honors the merged pair.
        let ecosystem = ReleaseConfig {
            tag_message: Some("release {version}".into()),
            ..ReleaseConfig::default()
        };
        let module = ReleaseConfig {
            sign_tags: Some(true),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, Some(&module)).unwrap();
        assert!(resolved.sign_tags);
        assert_eq!(resolved.tag_message.as_deref(), Some("release {version}"));
    }

    #[test]
    fn sign_format_and_key_resolve_onto_the_backend_enum() {
        let ecosystem = ReleaseConfig {
            tag_message: Some("release {version}".into()),
            sign_tags: Some(true),
            sign_format: Some("ssh".into()),
            signing_key: Some("KEYID".into()),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert_eq!(resolved.sign_format, Some(toven_ports::SignFormat::Ssh));
        assert_eq!(resolved.signing_key.as_deref(), Some("KEYID"));
    }

    #[test]
    fn sign_format_accepts_the_gpg_alias_case_insensitively() {
        let ecosystem = ReleaseConfig {
            tag_message: Some("release {version}".into()),
            sign_tags: Some(true),
            sign_format: Some("GPG".into()),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert_eq!(resolved.sign_format, Some(toven_ports::SignFormat::OpenPgp));
    }

    #[test]
    fn unknown_sign_format_is_rejected() {
        let ecosystem = ReleaseConfig {
            tag_message: Some("release {version}".into()),
            sign_tags: Some(true),
            sign_format: Some("pkcs11".into()),
            ..ReleaseConfig::default()
        };
        let error = ResolvedReleaseSettings::resolve(&ecosystem, None)
            .expect_err("unknown signing backend");
        assert!(error.to_string().contains("sign_format"), "{error}");
    }

    #[test]
    fn sign_format_or_key_without_sign_tags_is_rejected() {
        let ecosystem = ReleaseConfig {
            sign_format: Some("ssh".into()),
            ..ReleaseConfig::default()
        };
        let error = ResolvedReleaseSettings::resolve(&ecosystem, None)
            .expect_err("signing material requires sign_tags");
        assert!(error.to_string().contains("sign_tags"), "{error}");
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
    fn push_policy_defaults_to_branch_and_tags_and_resolves_from_config() {
        let tags_only = ReleaseConfig {
            push_branch: Some(false),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&tags_only, None).unwrap();
        assert_eq!(resolved.push, PushPolicy::TagsOnly);
        // `push = false` disables the push entirely, whatever `push_branch` says.
        let disabled = ReleaseConfig {
            push: Some(false),
            push_branch: Some(false),
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&disabled, None).unwrap();
        assert_eq!(resolved.push, PushPolicy::Disabled);
        // The module override narrows the ecosystem default to tags-only.
        let inherited = ResolvedReleaseSettings::resolve(
            &ReleaseConfig::default(),
            Some(&ReleaseConfig {
                push_branch: Some(false),
                ..ReleaseConfig::default()
            }),
        )
        .unwrap();
        assert_eq!(inherited.push, PushPolicy::TagsOnly);
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
    fn visibility_defaults_to_public_and_a_module_override_wins() {
        use toven_ports::Visibility;

        // Unset everywhere: releases are public.
        let default = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert_eq!(default.visibility, Visibility::Public);

        // An ecosystem-level visibility is inherited by a module that omits it.
        let ecosystem = ReleaseConfig {
            visibility: Some(Visibility::Internal),
            ..ReleaseConfig::default()
        };
        let inherited = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();
        assert_eq!(inherited.visibility, Visibility::Internal);

        // A module override replaces the inherited exposure (override-wins merge).
        let module = ReleaseConfig {
            visibility: Some(Visibility::Private),
            ..ReleaseConfig::default()
        };
        let overridden = ResolvedReleaseSettings::resolve(&ecosystem, Some(&module)).unwrap();
        assert_eq!(overridden.visibility, Visibility::Private);
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

    #[test]
    fn an_unconfigured_phase_resolves_native() {
        use toven_model::ReleasePhase;
        use toven_ports::PhaseBacking;

        let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
        assert_eq!(
            resolved.phase_backing(ReleasePhase::Package).unwrap(),
            PhaseBacking::Native
        );
        assert!(resolved.delegated_tool(ReleasePhase::Package).is_none());
    }

    #[test]
    fn a_configured_phase_resolves_delegated_with_its_tool() {
        use std::collections::BTreeMap;

        use toven_model::ReleasePhase;
        use toven_ports::{
            DelegatedTool, PhaseBacking, PhaseBackingKind, PhaseConfig, PhasesConfig,
        };

        let mut phases = BTreeMap::new();
        phases.insert(
            ReleasePhase::Package,
            PhaseConfig {
                backing: PhaseBackingKind::Delegated,
                delegated: Some(DelegatedTool {
                    tool: "goreleaser".into(),
                    args: Some(vec!["release".into()]),
                    preview: vec!["release".into(), "--snapshot".into()],
                }),
            },
        );
        let ecosystem = ReleaseConfig {
            phases: Some(PhasesConfig(phases)),
            ..ReleaseConfig::default()
        };

        let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

        assert_eq!(
            resolved.phase_backing(ReleasePhase::Package).unwrap(),
            PhaseBacking::delegated("goreleaser")
        );
        let tool = resolved
            .delegated_tool(ReleasePhase::Package)
            .expect("delegated tool");
        assert_eq!(tool.tool, "goreleaser");
        // A phase with no entry still resolves native alongside the delegated one.
        assert_eq!(
            resolved.phase_backing(ReleasePhase::Publish).unwrap(),
            PhaseBacking::Native
        );
    }
}
