//! Field-merge: fold a per-module release override onto an ecosystem-default
//! [`ReleaseConfig`].

use crate::config::ReleaseConfig;

/// Field-merge a per-module release `over`ride onto an ecosystem `base` config.
///
/// Every field is presence-aware: a `Some` override field **replaces** the base
/// value for exactly that field, and a `None` inherits the base. So a
/// per-module `[modules.<name>.release]` that only sets `level` flips one field
/// while the rest carry over, and it can explicitly **clear** a base default —
/// e.g. `branches = []` opts one module out of the ecosystem's branch
/// restriction. This matches the documented precedence (per-module > ecosystem >
/// adapter default).
#[must_use]
pub fn merge_release(base: &ReleaseConfig, over: &ReleaseConfig) -> ReleaseConfig {
    let mut merged = base.clone();

    if over.strategy.is_some() {
        merged.strategy.clone_from(&over.strategy);
    }
    if over.level.is_some() {
        merged.level = over.level;
    }
    if over.dependent_version.is_some() {
        merged.dependent_version = over.dependent_version;
    }
    if over.prerelease.is_some() {
        merged.prerelease.clone_from(&over.prerelease);
    }
    if over.tag_format.is_some() {
        merged.tag_format.clone_from(&over.tag_format);
    }
    if over.tag_message.is_some() {
        merged.tag_message.clone_from(&over.tag_message);
    }
    if over.sign_tags.is_some() {
        merged.sign_tags = over.sign_tags;
        if over.sign_tags == Some(false) {
            merged.sign_format = None;
            merged.signing_key = None;
        }
    }
    if over.sign_format.is_some() {
        merged.sign_format.clone_from(&over.sign_format);
    }
    if over.signing_key.is_some() {
        merged.signing_key.clone_from(&over.signing_key);
    }
    if over.commit_message.is_some() {
        merged.commit_message.clone_from(&over.commit_message);
    }
    if over.changelog.is_some() {
        merged.changelog.clone_from(&over.changelog);
    }
    if over.push.is_some() {
        merged.push = over.push;
    }
    if over.push_branch.is_some() {
        merged.push_branch = over.push_branch;
    }
    if over.remote.is_some() {
        merged.remote.clone_from(&over.remote);
    }
    if over.branches.is_some() {
        merged.branches.clone_from(&over.branches);
    }
    if over.registry.is_some() {
        merged.registry.clone_from(&over.registry);
    }
    if over.publish.is_some() {
        merged.publish = over.publish;
    }
    if over.exclude.is_some() {
        merged.exclude = over.exclude;
    }
    if over.offline.is_some() {
        merged.offline = over.offline;
    }
    if over.token_env.is_some() {
        merged.token_env.clone_from(&over.token_env);
    }
    if over.visibility.is_some() {
        merged.visibility = over.visibility;
    }
    if over.sign.is_some() {
        merged.sign.clone_from(&over.sign);
    }
    if over.readiness.is_some() {
        merged.readiness.clone_from(&over.readiness);
    }
    if over.host.is_some() {
        merged.host.clone_from(&over.host);
    }
    if over.image.is_some() {
        merged.image.clone_from(&over.image);
    }
    if over.phases.is_some() {
        merged.phases.clone_from(&over.phases);
    }
    if over.entrypoint.is_some() {
        merged.entrypoint = over.entrypoint;
    }
    if over.umbrella.is_some() {
        merged.umbrella = over.umbrella;
    }
    if over.version_references.is_some() {
        merged
            .version_references
            .clone_from(&over.version_references);
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::merge_release;
    use crate::config::{BumpLevel, ChangelogConfig, ReleaseConfig};

    fn base() -> ReleaseConfig {
        ReleaseConfig {
            strategy: Some("semver-cascade".into()),
            level: Some(BumpLevel::Patch),
            registry: Some("crates-io".into()),
            branches: Some(vec!["main".into()]),
            ..ReleaseConfig::default()
        }
    }

    #[test]
    fn set_override_field_replaces_and_rest_inherits() {
        let over = ReleaseConfig {
            level: Some(BumpLevel::Minor),
            strategy: Some("manifest".into()),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        assert_eq!(merged.level, Some(BumpLevel::Minor));
        // a per-module strategy override replaces the ecosystem default:
        assert_eq!(merged.strategy.as_deref(), Some("manifest"));
        // inherited from base, untouched by the override:
        assert_eq!(merged.registry.as_deref(), Some("crates-io"));
        assert_eq!(merged.branches.as_deref(), Some(["main".into()].as_slice()));
    }

    #[test]
    fn non_default_sub_config_replaces() {
        let over = ReleaseConfig {
            changelog: Some(ChangelogConfig {
                required: true,
                ..ChangelogConfig::default()
            }),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        assert!(merged.changelog.expect("changelog set").required);
    }

    #[test]
    fn host_override_replaces_base_host() {
        use crate::config::HostConfig;

        let base = ReleaseConfig {
            host: Some(HostConfig {
                forge: Some("github".into()),
                draft: Some(true),
                ..HostConfig::default()
            }),
            ..ReleaseConfig::default()
        };
        let over = ReleaseConfig {
            host: Some(HostConfig {
                forge: Some("github".into()),
                prerelease: Some(true),
                ..HostConfig::default()
            }),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base, &over);

        let host = merged.host.expect("host set");
        // The whole host sub-config is replaced, not field-merged.
        assert_eq!(host.forge.as_deref(), Some("github"));
        assert_eq!(host.draft, None);
        assert_eq!(host.prerelease, Some(true));
    }

    #[test]
    fn disabling_signed_tags_clears_inherited_signing_material() {
        let base = ReleaseConfig {
            sign_tags: Some(true),
            sign_format: Some("ssh".into()),
            signing_key: Some("KEYID".into()),
            ..ReleaseConfig::default()
        };
        let over = ReleaseConfig {
            sign_tags: Some(false),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base, &over);

        assert_eq!(merged.sign_tags, Some(false));
        assert_eq!(merged.sign_format, None);
        assert_eq!(merged.signing_key, None);
    }

    #[test]
    fn explicit_signing_material_with_disabled_signing_stays_visible_to_validation() {
        let base = ReleaseConfig {
            sign_tags: Some(true),
            sign_format: Some("ssh".into()),
            signing_key: Some("KEYID".into()),
            ..ReleaseConfig::default()
        };
        let over = ReleaseConfig {
            sign_tags: Some(false),
            sign_format: Some("openpgp".into()),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base, &over);

        assert_eq!(merged.sign_tags, Some(false));
        assert_eq!(merged.sign_format.as_deref(), Some("openpgp"));
        assert_eq!(merged.signing_key, None);
    }

    #[test]
    fn image_override_replaces_base_image() {
        use crate::config::ImageConfig;

        let base = ReleaseConfig {
            image: Some(ImageConfig {
                registry: "ghcr.io/acme".into(),
                name: "base".into(),
                ..ImageConfig::default()
            }),
            ..ReleaseConfig::default()
        };
        let over = ReleaseConfig {
            image: Some(ImageConfig {
                registry: "ghcr.io/acme".into(),
                name: "app".into(),
                ..ImageConfig::default()
            }),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base, &over);

        let image = merged.image.expect("image set");
        assert_eq!(image.name, "app");
    }

    #[test]
    fn empty_override_inherits_base_entirely() {
        let merged = merge_release(&base(), &ReleaseConfig::default());
        assert_eq!(merged, base());
    }

    #[test]
    fn override_entrypoint_and_umbrella_replace_base() {
        use toven_model::Entrypoint;

        let base = ReleaseConfig {
            entrypoint: Some(Entrypoint::Toven),
            ..ReleaseConfig::default()
        };
        let over = ReleaseConfig {
            entrypoint: Some(Entrypoint::Maintainer),
            umbrella: Some(true),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base, &over);

        assert_eq!(merged.entrypoint, Some(Entrypoint::Maintainer));
        assert_eq!(merged.umbrella, Some(true));
    }

    #[test]
    fn unset_entrypoint_override_inherits_base() {
        use toven_model::Entrypoint;

        let base = ReleaseConfig {
            entrypoint: Some(Entrypoint::Maintainer),
            umbrella: Some(true),
            ..ReleaseConfig::default()
        };
        let merged = merge_release(&base, &ReleaseConfig::default());
        assert_eq!(merged.entrypoint, Some(Entrypoint::Maintainer));
        assert_eq!(merged.umbrella, Some(true));
    }

    #[test]
    fn non_empty_override_list_replaces_base_list() {
        let over = ReleaseConfig {
            branches: Some(vec!["release".into(), "next".into()]),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        assert_eq!(
            merged.branches.as_deref(),
            Some(["release".into(), "next".into()].as_slice())
        );
    }

    #[test]
    fn empty_override_list_clears_base_default() {
        let over = ReleaseConfig {
            branches: Some(Vec::new()),
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        // an explicit `branches = []` clears the ecosystem's branch restriction:
        assert_eq!(merged.branches.as_deref(), Some([].as_slice()));
    }
}
