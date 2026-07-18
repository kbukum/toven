//! Field-merge: fold a per-module release override onto an ecosystem-default
//! [`ReleaseConfig`].

use crate::config::ReleaseConfig;

/// Field-merge a per-module release `over`ride onto an ecosystem `base` config.
///
/// Every field is presence-aware: a `Some` override field **replaces** the base
/// value for exactly that field, and a `None` inherits the base. So a per-module
/// `[modules.<name>.release]` that only sets `level` flips one field while the
/// rest carry over, and it can explicitly **clear** a base default — e.g.
/// `branches = []` opts one module out of the ecosystem's branch restriction.
/// This matches the documented precedence (per-module > ecosystem > adapter
/// default).
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
    if over.commit_message.is_some() {
        merged.commit_message.clone_from(&over.commit_message);
    }
    if over.changelog.is_some() {
        merged.changelog.clone_from(&over.changelog);
    }
    if over.push.is_some() {
        merged.push = over.push;
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
    if over.offline.is_some() {
        merged.offline = over.offline;
    }
    if over.token_env.is_some() {
        merged.token_env.clone_from(&over.token_env);
    }
    if over.sign.is_some() {
        merged.sign.clone_from(&over.sign);
    }
    if over.readiness.is_some() {
        merged.readiness.clone_from(&over.readiness);
    }
    if over.hooks.is_some() {
        merged.hooks.clone_from(&over.hooks);
    }
    if over.host.is_some() {
        merged.host.clone_from(&over.host);
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
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        assert_eq!(merged.level, Some(BumpLevel::Minor));
        // inherited from base, untouched by the override:
        assert_eq!(merged.strategy.as_deref(), Some("semver-cascade"));
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
    fn empty_override_inherits_base_entirely() {
        let merged = merge_release(&base(), &ReleaseConfig::default());
        assert_eq!(merged, base());
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
