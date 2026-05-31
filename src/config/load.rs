//! Config loading and normalization orchestration.

use std::path::Path;

use crate::{
    config::{
        ConfigDocument,
        dependency::normalize_dependency_overlays,
        profile::{attach_scope_overrides, normalize_profiles},
        project::{build_workspace, normalize_project_config},
        scope::model::normalize_scope_overrides,
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
    let project = normalize_project_config(document.project, config_path)?;
    let resolver = PresetResolver::new(project.root.clone());
    normalize_resolved_config(
        document.profiles,
        document.scopes,
        document.overlays,
        project,
        &resolver,
    )
}

#[cfg(test)]
fn normalize_config_with_resolver(
    document: ConfigDocument,
    config_path: &Path,
    resolver: &PresetResolver,
) -> AppResult<Workspace> {
    let project = normalize_project_config(document.project, config_path)?;
    normalize_resolved_config(
        document.profiles,
        document.scopes,
        document.overlays,
        project,
        resolver,
    )
}

fn normalize_resolved_config(
    profile_configs: std::collections::BTreeMap<String, crate::config::ProfileConfig>,
    scope_configs: std::collections::BTreeMap<String, crate::config::ScopeConfig>,
    overlay_configs: Vec<crate::config::DependencyOverlayConfig>,
    project: crate::config::project::NormalizedProject,
    resolver: &PresetResolver,
) -> AppResult<Workspace> {
    let mut profiles = normalize_profiles(profile_configs, &project.root, resolver)?;
    let profile_adapters = profiles
        .iter()
        .map(|profile| (profile.name.clone(), profile.language.clone()))
        .collect();
    let scope_overrides = normalize_scope_overrides(scope_configs, &profile_adapters, resolver)?;
    let dependency_overlays = normalize_dependency_overlays(overlay_configs)?;
    attach_scope_overrides(&mut profiles, scope_overrides)?;
    let mut workspace = build_workspace(project, profiles);
    workspace.dependency_overlays = dependency_overlays;
    Ok(workspace)
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
        assert!(!workspace.profiles[0].tasks[0].cache_args);
        assert!(matches!(
            workspace.profiles[0].tasks[0].command,
            TaskCommand::Argv(_)
        ));
    }

    #[test]
    fn loads_task_cache_args_flag() {
        let root = rskit_testutil::test_workspace!("cache-args-config");
        let config_path = root
            .copy_fixture("config/cache-args.toml", "toven.toml")
            .expect("copy config fixture");

        let workspace = load_workspace(&config_path).expect("config loads");

        assert!(workspace.profiles[0].tasks[0].cache_args);
    }

    #[test]
    fn loads_task_shared_inputs() {
        let root = rskit_testutil::test_workspace!("shared-inputs-config");
        let config_path = root
            .copy_fixture("config/shared-inputs.toml", "toven.toml")
            .expect("copy config fixture");

        let workspace = load_workspace(&config_path).expect("config loads");

        assert_eq!(
            workspace.profiles[0].tasks[0].shared_inputs,
            ["Cargo.lock", "rust-toolchain.toml"]
        );
    }

    #[test]
    fn rejects_template_shared_inputs() {
        let root = rskit_testutil::test_workspace!("template-shared-input");
        let config_path = root
            .copy_fixture("config/template-shared-input.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("template shared input should fail");

        assert!(
            error
                .message
                .contains("profiles.rust.tasks.test.shared_inputs")
        );
        assert!(error.message.contains("do not support templates"));
    }

    #[test]
    fn rejects_current_dir_shared_inputs() {
        let root = rskit_testutil::test_workspace!("current-dir-shared-input");
        let config_path = root
            .copy_fixture("config/current-dir-shared-input.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("current-dir shared input should fail");

        assert!(
            error
                .message
                .contains("profiles.rust.tasks.test.shared_inputs")
        );
        assert!(error.message.contains("cannot contain '.' components"));
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
    fn reports_invalid_profile_name_field() {
        let root = rskit_testutil::test_workspace!("invalid-profile-name");
        let config_path = root
            .copy_fixture("config/invalid-profile-name.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid profile name should fail");

        assert!(error.message.contains("profiles.bad/name"));
    }

    #[test]
    fn reports_invalid_profile_language_field() {
        let root = rskit_testutil::test_workspace!("invalid-profile-language");
        let config_path = root
            .copy_fixture("config/invalid-profile-language.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid profile language should fail");

        assert!(error.message.contains("profiles.rust.adapter"));
    }

    #[test]
    fn reports_invalid_task_name_field() {
        let root = rskit_testutil::test_workspace!("invalid-task-name");
        let config_path = root
            .copy_fixture("config/invalid-task-name.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid task name should fail");

        assert!(error.message.contains("profiles.rust.tasks.bad/task"));
    }

    #[test]
    fn reports_invalid_module_arg_template_field() {
        let root = rskit_testutil::test_workspace!("invalid-module-arg-template");
        let config_path = root
            .copy_fixture("config/invalid-module-arg-template.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("invalid module template should fail");

        assert!(error.message.contains("profiles.rust.module_arg_template"));
        assert!(error.message.contains("unknown placeholder"));
    }

    #[test]
    fn reports_invalid_resource_group_field() {
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
    fn rejects_nonpersistent_ready_timeout() {
        let root = rskit_testutil::test_workspace!("nonpersistent-ready-timeout");
        let config_path = root
            .copy_fixture("config/nonpersistent-ready-timeout.toml", "toven.toml")
            .expect("copy config fixture");

        let error = load_workspace(&config_path).expect_err("ready timeout requires persistence");

        assert!(
            error
                .message
                .contains("profiles.rust.tasks.test.ready_timeout_seconds")
        );
        assert!(
            error
                .message
                .contains("ready_timeout_seconds requires persistent = true")
        );
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

        assert!(error.message.contains("profiles.rust.tasks.test.preset"));
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

    #[test]
    fn loads_dependency_overlays() {
        let root = rskit_testutil::test_workspace!("dependency-overlays");
        let config_path = root
            .copy_fixture("config/dependency-overlays.toml", "toven.toml")
            .expect("copy config fixture");

        let workspace = load_workspace(&config_path).expect("config loads");

        assert_eq!(workspace.dependency_overlays.len(), 1);
        let overlay = &workspace.dependency_overlays[0];
        assert_eq!(overlay.from.0, "app");
        assert_eq!(overlay.from.1.as_str(), "api");
        assert_eq!(overlay.to.0, "lib");
        assert_eq!(overlay.to.1.as_str(), "shared");
    }
}
