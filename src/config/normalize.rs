//! Conversion from strict config documents to core planning contracts.

use std::path::{Path, PathBuf};

use crate::{
    config::{ConfigDocument, ProfileConfig, TaskConfig},
    core::{AppError, AppResult, ExecutionMode, Profile, Task, TaskCommand, Workspace},
    preset::PresetResolver,
    validation::{
        validate_command_template, validate_identifier, validate_name, validate_template,
        validate_templates,
    },
};

const SUPPORTED_SCHEMA: u16 = 1;
const DEFAULT_RESOURCE_GROUP: &str = "{workspace.root}";

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
    let schema = validate_schema(document.workspace.schema)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let root = normalize_root(config_dir, document.workspace.root.as_deref())?;
    let resolver = PresetResolver::new(root.clone());
    normalize_resolved_config(document, schema, root, &resolver)
}

#[cfg(test)]
fn normalize_config_with_resolver(
    document: ConfigDocument,
    config_path: &Path,
    resolver: &PresetResolver,
) -> AppResult<Workspace> {
    let schema = validate_schema(document.workspace.schema)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let root = normalize_root(config_dir, document.workspace.root.as_deref())?;
    normalize_resolved_config(document, schema, root, resolver)
}

fn normalize_resolved_config(
    document: ConfigDocument,
    schema: u16,
    root: PathBuf,
    resolver: &PresetResolver,
) -> AppResult<Workspace> {
    let name = match document.workspace.name {
        Some(name) => {
            validate_name("workspace.name", &name)?;
            name
        }
        None => root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("workspace")
            .to_string(),
    };

    if document.profiles.is_empty() {
        return Err(AppError::invalid_input(
            "profiles",
            "at least one profile is required",
        ));
    }

    let mut profiles = Vec::with_capacity(document.profiles.len());
    for (profile_name, profile) in document.profiles {
        profiles.push(normalize_profile(profile_name, profile, resolver)?);
    }

    Ok(Workspace {
        schema,
        name,
        root,
        profiles,
    })
}

fn validate_schema(schema: Option<u16>) -> AppResult<u16> {
    let schema = schema.unwrap_or(SUPPORTED_SCHEMA);
    if schema != SUPPORTED_SCHEMA {
        return Err(AppError::invalid_input(
            "workspace.schema",
            format!("unsupported schema {schema}; supported schema is {SUPPORTED_SCHEMA}"),
        ));
    }
    Ok(schema)
}

fn normalize_root(config_dir: &Path, root: Option<&Path>) -> AppResult<PathBuf> {
    let root = root.unwrap_or_else(|| Path::new("."));
    let root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        config_dir.join(root)
    };
    rskit_fs::canonicalize(&root).map_err(|error| {
        AppError::invalid_input(
            "workspace.root",
            format!("failed to resolve workspace root '{}'", root.display()),
        )
        .with_cause(error)
    })
}

fn normalize_profile(
    name: String,
    config: ProfileConfig,
    resolver: &PresetResolver,
) -> AppResult<Profile> {
    validate_identifier("profiles", &name)?;
    validate_identifier("profiles.language", &config.language)?;
    if let Some(discovery_command) = &config.discovery_command {
        validate_command_template(
            format!("profiles.{name}.discovery_command"),
            discovery_command,
        )?;
    }

    if config.tasks.is_empty() {
        return Err(AppError::invalid_input(
            format!("profiles.{name}.tasks"),
            "at least one task is required",
        ));
    }

    let module_arg_template = config.module_arg_template.unwrap_or_default();
    validate_templates(
        format!("profiles.{name}.module_arg_template"),
        &module_arg_template,
    )?;

    let resource_group = config
        .resource_group
        .unwrap_or_else(|| DEFAULT_RESOURCE_GROUP.to_string());
    validate_template(format!("profiles.{name}.resource_group"), &resource_group)?;

    let mut tasks = Vec::with_capacity(config.tasks.len());
    for (task_name, task) in config.tasks {
        tasks.push(normalize_task(
            &name,
            &config.language,
            task_name,
            task,
            resolver,
        )?);
    }

    Ok(Profile {
        name,
        language: config.language,
        discovery_command: config.discovery_command,
        execution: config.execution.unwrap_or(ExecutionMode::SpawnEach),
        module_arg_template,
        resource_group,
        tasks,
    })
}

fn normalize_task(
    profile_name: &str,
    language: &str,
    name: String,
    config: TaskConfig,
    resolver: &PresetResolver,
) -> AppResult<Task> {
    validate_identifier("tasks", &name)?;

    let command = match (config.argv, config.preset) {
        (Some(argv), None) => {
            validate_command_template(format!("profiles.{profile_name}.tasks.{name}.argv"), &argv)?;
            TaskCommand::Argv(argv)
        }
        (None, Some(preset)) => {
            validate_identifier(
                format!("profiles.{profile_name}.tasks.{name}.preset"),
                &preset,
            )?;
            TaskCommand::ResolvedPreset(resolver.resolve(language, &preset)?)
        }
        (Some(_), Some(_)) => {
            return Err(AppError::invalid_input(
                format!("profiles.{profile_name}.tasks.{name}"),
                "task must define either 'argv' or 'preset', not both",
            ));
        }
        (None, None) => {
            return Err(AppError::invalid_input(
                format!("profiles.{profile_name}.tasks.{name}"),
                "task must define either 'argv' or 'preset'",
            ));
        }
    };

    Ok(Task { name, command })
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
