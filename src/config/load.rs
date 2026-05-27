//! Config loading and normalization orchestration.

use std::path::Path;

use crate::{
    config::{
        ConfigDocument,
        profile::normalize_profiles,
        workspace::{build_workspace, normalize_workspace_config},
    },
    core::{AppResult, Workspace},
    preset::PresetResolver,
};

/// Load and normalize a `toven.toml` file.
pub fn load_workspace(path: impl AsRef<Path>) -> AppResult<Workspace> {
    let path = path.as_ref();
    let document = rskit_config::ConfigLoader::toml(path).load::<ConfigDocument>()?;
    normalize_config(document, path)
}

/// Normalize a config document loaded from `config_path`.
pub fn normalize_config(
    document: ConfigDocument,
    config_path: impl AsRef<Path>,
) -> AppResult<Workspace> {
    let config_path = config_path.as_ref();
    let workspace = normalize_workspace_config(document.workspace, config_path)?;
    let resolver = PresetResolver::new(workspace.root.clone());
    normalize_resolved_config(document.profiles, workspace, &resolver)
}

#[cfg(test)]
fn normalize_config_with_resolver(
    document: ConfigDocument,
    config_path: &Path,
    resolver: &PresetResolver,
) -> AppResult<Workspace> {
    let workspace = normalize_workspace_config(document.workspace, config_path)?;
    normalize_resolved_config(document.profiles, workspace, resolver)
}

fn normalize_resolved_config(
    profiles: std::collections::BTreeMap<String, crate::config::ProfileConfig>,
    workspace: crate::config::workspace::NormalizedWorkspace,
    resolver: &PresetResolver,
) -> AppResult<Workspace> {
    let profiles = normalize_profiles(profiles, resolver)?;
    Ok(build_workspace(workspace, profiles))
}

#[cfg(test)]
mod tests {
    use crate::{
        config::load_workspace,
        core::{ExecutionMode, TaskCommand},
        preset::PresetResolver,
    };

    use super::{ConfigDocument, normalize_config_with_resolver};

    #[test]
    fn loads_direct_argv_config() {
        let root = rskit_testutil::test_workspace!("direct");
        let config_path = root
            .copy_fixture("config/direct-argv.toml", "toven.toml")
            .expect("copy config fixture");

        let workspace = load_workspace(&config_path).expect("config loads");

        assert_eq!(workspace.name, "demo");
        assert_eq!(workspace.profiles[0].name, "rust");
        assert_eq!(workspace.profiles[0].execution, ExecutionMode::BatchReady);
        assert_eq!(workspace.profiles[0].tasks[0].name, "test");
        assert!(matches!(
            workspace.profiles[0].tasks[0].command,
            TaskCommand::Argv(_)
        ));
    }

    #[test]
    fn rejects_unknown_top_level_fields() {
        let root = rskit_testutil::test_workspace!("unknown");
        let config_path = root
            .copy_fixture("config/unknown-top-level.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("unknown field should fail");

        assert!(error.message.contains("unknown"));
    }

    #[test]
    fn rejects_unknown_nested_fields() {
        let root = rskit_testutil::test_workspace!("unknown-nested");
        let config_path = root
            .copy_fixture("config/unknown-nested.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("unknown nested field should fail");

        assert!(error.message.contains("unknown"));
    }

    #[test]
    fn sorts_profiles_and_tasks_by_name() {
        let root = rskit_testutil::test_workspace!("sorted");
        let config_path = root
            .copy_fixture("config/sorted.toml", "toven.toml")
            .expect("copy config fixture");

        let workspace = load_workspace(&config_path).expect("config loads");

        assert_eq!(workspace.profiles[0].name, "alpha");
        assert_eq!(workspace.profiles[1].name, "zed");
        assert_eq!(workspace.profiles[1].tasks[0].name, "a-first");
        assert_eq!(workspace.profiles[1].tasks[1].name, "z-last");
    }

    #[test]
    fn reports_invalid_profile_name_with_config_path() {
        let root = rskit_testutil::test_workspace!("invalid-profile-name");
        let config_path = root
            .copy_fixture("config/invalid-profile-name.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid profile name should fail");

        assert!(error.message.contains("profiles.bad/name"));
    }

    #[test]
    fn reports_invalid_profile_language_with_config_path() {
        let root = rskit_testutil::test_workspace!("invalid-profile-language");
        let config_path = root
            .copy_fixture("config/invalid-profile-language.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid profile language should fail");

        assert!(error.message.contains("profiles.rust.language"));
    }

    #[test]
    fn reports_invalid_task_name_with_config_path() {
        let root = rskit_testutil::test_workspace!("invalid-task-name");
        let config_path = root
            .copy_fixture("config/invalid-task-name.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid task name should fail");

        assert!(error.message.contains("profiles.rust.tasks.bad/task"));
    }

    #[test]
    fn reports_invalid_module_arg_template_with_config_path() {
        let root = rskit_testutil::test_workspace!("invalid-module-arg-template");
        let config_path = root
            .copy_fixture("config/invalid-module-arg-template.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid module template should fail");

        assert!(error.message.contains("profiles.rust.module_arg_template"));
        assert!(error.message.contains("unknown placeholder"));
    }

    #[test]
    fn reports_invalid_resource_group_with_config_path() {
        let root = rskit_testutil::test_workspace!("invalid-resource-group");
        let config_path = root
            .copy_fixture("config/invalid-resource-group.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid resource group should fail");

        assert!(error.message.contains("profiles.rust.resource_group"));
        assert!(error.message.contains("unknown placeholder"));
    }

    #[test]
    fn rejects_empty_task_argv() {
        let root = rskit_testutil::test_workspace!("empty-task-argv");
        let config_path = root
            .copy_fixture("config/empty-task-argv.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("empty argv should fail");

        assert!(error.message.contains("profiles.rust.tasks.test.argv"));
        assert!(error.message.contains("at least one argv item is required"));
    }

    #[test]
    fn rejects_task_with_argv_and_preset() {
        let root = rskit_testutil::test_workspace!("task-both-argv-preset");
        let config_path = root
            .copy_fixture("config/task-both-argv-preset.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("ambiguous task command should fail");

        assert!(error.message.contains("profiles.rust.tasks.test"));
        assert!(
            error
                .message
                .contains("either 'argv' or 'preset', not both")
        );
    }

    #[test]
    fn rejects_task_without_argv_or_preset() {
        let root = rskit_testutil::test_workspace!("task-missing-command");
        let config_path = root
            .copy_fixture("config/task-missing-command.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("missing task command should fail");

        assert!(error.message.contains("profiles.rust.tasks.test"));
        assert!(error.message.contains("either 'argv' or 'preset'"));
    }

    #[test]
    fn resolves_project_local_presets() {
        let root = rskit_testutil::test_workspace!("preset");
        let config_path = root
            .copy_fixture("config/preset-workspace.toml", "toven.toml")
            .expect("copy config fixture");
        root.copy_fixture(
            "presets/cargo-nextest.toml",
            ".toven/lang/rust/presets/cargo-nextest.toml",
        )
        .expect("copy preset fixture");

        let workspace = load_workspace(&config_path).expect("config loads");

        let TaskCommand::ResolvedPreset(preset) = &workspace.profiles[0].tasks[0].command else {
            panic!("preset should resolve");
        };
        assert_eq!(preset.name, "cargo-nextest");
        assert_eq!(preset.argv[0], "cargo");
    }

    #[test]
    fn reports_missing_preset() {
        let root = rskit_testutil::test_workspace!("missing-preset");
        let config_path = root
            .copy_fixture("config/missing-preset.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("missing preset should fail");

        assert!(error.message.contains("preset 'missing' not found"));
        assert!(
            error
                .message
                .contains(".toven/lang/rust/presets/missing.toml")
        );
    }

    #[test]
    fn reports_unsupported_schema_before_missing_root() {
        let root = rskit_testutil::test_workspace!("unsupported-schema");
        let config_path = root
            .copy_fixture("config/unsupported-schema-missing-root.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("unsupported schema should fail");

        assert!(error.message.contains("unsupported schema 2"));
    }

    #[test]
    fn rejects_preset_language_mismatch() {
        let root = rskit_testutil::test_workspace!("preset-mismatch");
        let config_path = root
            .copy_fixture("config/preset-check-workspace.toml", "toven.toml")
            .expect("copy config fixture");
        root.copy_fixture(
            "presets/check-language-mismatch.toml",
            ".toven/lang/rust/presets/check.toml",
        )
        .expect("copy preset fixture");

        let error = load_workspace(&config_path).expect_err("language mismatch should fail");

        assert!(error.message.contains("declares language 'go'"));
    }

    #[test]
    fn skips_user_preset_lookup_when_home_is_unset() {
        let root = rskit_testutil::test_workspace!("no-home");
        let config_path = root
            .copy_fixture("config/preset-check-workspace.toml", "toven.toml")
            .expect("copy config fixture");
        let document: ConfigDocument = rskit_config::ConfigLoader::toml(&config_path)
            .load()
            .expect("load config document fixture");
        let resolver = PresetResolver::new(root.path().to_path_buf()).without_user_home();

        let error = normalize_config_with_resolver(document, &config_path, &resolver)
            .expect_err("missing preset");

        assert!(error.message.contains("searched:"));
        assert!(!error.message.contains(", "));
    }
}
