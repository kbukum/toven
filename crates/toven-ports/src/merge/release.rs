//! Field-merge: fold a per-module release override onto an ecosystem-default
//! [`ReleaseConfig`].

use crate::config::ReleaseConfig;

/// Field-merge a per-module release `over`ride onto an ecosystem `base` config.
///
/// A set override field **replaces** the base: an `Option` that is `Some`, a
/// non-empty list (`branches`/`readiness`), or a non-default sub-config
/// (`prerelease`/`changelog`/`sign`/`hooks`) wins; anything the override leaves
/// unset inherits the base. So a per-module `[modules.<name>.release]` that only
/// sets `level` flips exactly one field while the rest carry over from
/// `[ecosystems.<id>].release`, matching the documented precedence
/// (per-module > ecosystem > adapter default).
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
    if !over.prerelease.is_default() {
        merged.prerelease = over.prerelease.clone();
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
    if !over.changelog.is_default() {
        merged.changelog = over.changelog.clone();
    }
    if over.push.is_some() {
        merged.push = over.push;
    }
    if over.remote.is_some() {
        merged.remote.clone_from(&over.remote);
    }
    if !over.branches.is_empty() {
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
    if !over.sign.is_default() {
        merged.sign = over.sign.clone();
    }
    if !over.readiness.is_empty() {
        merged.readiness.clone_from(&over.readiness);
    }
    if !over.hooks.is_default() {
        merged.hooks = over.hooks.clone();
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
            branches: vec!["main".into()],
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
        assert_eq!(merged.branches, ["main"]);
    }

    #[test]
    fn non_default_sub_config_replaces() {
        let over = ReleaseConfig {
            changelog: ChangelogConfig {
                required: true,
                ..ChangelogConfig::default()
            },
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        assert!(merged.changelog.required);
    }

    #[test]
    fn empty_override_inherits_base_entirely() {
        let merged = merge_release(&base(), &ReleaseConfig::default());
        assert_eq!(merged, base());
    }

    #[test]
    fn non_empty_override_list_replaces_base_list() {
        let over = ReleaseConfig {
            branches: vec!["release".into(), "next".into()],
            ..ReleaseConfig::default()
        };

        let merged = merge_release(&base(), &over);

        assert_eq!(merged.branches, ["release", "next"]);
    }
}
