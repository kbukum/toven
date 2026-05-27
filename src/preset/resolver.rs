//! Resolves external preset TOML files into core preset definitions.

use std::path::{Path, PathBuf};

use crate::{
    core::{AppError, AppResult, PresetDefinition},
    validation::{validate_command_template, validate_identifier},
};

/// Filesystem preset resolver.
#[derive(Debug, Clone)]
pub struct PresetResolver {
    project_root: PathBuf,
    user_home: Option<PathBuf>,
}

impl PresetResolver {
    /// Create a resolver for a workspace root.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            user_home: default_user_home(),
        }
    }

    /// Override the user home used for user-installed presets.
    #[must_use]
    pub fn with_user_home(mut self, user_home: impl Into<PathBuf>) -> Self {
        self.user_home = Some(user_home.into());
        self
    }

    /// Disable user-installed preset lookup.
    #[must_use]
    pub fn without_user_home(mut self) -> Self {
        self.user_home = None;
        self
    }

    /// Resolve a preset for `language` and `name`.
    pub fn resolve(&self, language: &str, name: &str) -> AppResult<PresetDefinition> {
        validate_identifier("preset.language", language)?;
        validate_identifier("preset.name", name)?;

        let searched = self.search_paths(language, name);
        let Some(path) = first_existing_file(&searched)? else {
            return Err(AppError::invalid_input(
                "preset",
                format!(
                    "preset '{name}' not found for language '{language}'; searched: {}",
                    searched
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };

        let file = rskit_config::ConfigLoader::toml(path).load::<RawPresetFile>()?;
        let preset = file.preset.into_definition();
        validate_preset(path, language, name, &preset)?;
        Ok(preset)
    }

    fn search_paths(&self, language: &str, name: &str) -> Vec<PathBuf> {
        let mut paths = vec![preset_path(&self.project_root, language, name)];
        if let Some(user_home) = &self.user_home
            && !user_home.as_os_str().is_empty()
        {
            paths.push(preset_path(user_home, language, name));
        }
        paths
    }
}

fn preset_path(root: &Path, language: &str, name: &str) -> PathBuf {
    root.join(".toven")
        .join("lang")
        .join(language)
        .join("presets")
        .join(format!("{name}.toml"))
}

fn first_existing_file(paths: &[PathBuf]) -> AppResult<Option<&PathBuf>> {
    for path in paths {
        if rskit_fs::sync_io::file::exists(path)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn validate_preset(
    path: &Path,
    language: &str,
    name: &str,
    preset: &PresetDefinition,
) -> AppResult<()> {
    if preset.name != name {
        return Err(AppError::invalid_input(
            "preset.name",
            format!(
                "preset file '{}' declares name '{}' but was requested as '{name}'",
                path.display(),
                preset.name
            ),
        ));
    }
    if preset.language != language {
        return Err(AppError::invalid_input(
            "preset.language",
            format!(
                "preset file '{}' declares language '{}' but was requested for '{language}'",
                path.display(),
                preset.language
            ),
        ));
    }
    validate_command_template(format!("preset '{}'.argv", path.display()), &preset.argv)?;
    Ok(())
}

fn default_user_home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPresetFile {
    preset: RawPresetDefinition,
}

impl rskit_validation::Validate for RawPresetFile {
    fn validate(&self) -> Result<(), rskit_validation::validator::ValidationErrors> {
        Ok(())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPresetDefinition {
    name: String,
    language: String,
    argv: Vec<String>,
    #[serde(default)]
    shared_inputs: Vec<String>,
}

impl RawPresetDefinition {
    fn into_definition(self) -> PresetDefinition {
        PresetDefinition {
            name: self.name,
            language: self.language,
            argv: self.argv,
            shared_inputs: self.shared_inputs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PresetResolver;

    #[test]
    fn project_preset_takes_precedence_over_user_preset() {
        let root = rskit_testutil::test_workspace!("precedence-root");
        let home = rskit_testutil::test_workspace!("precedence-home");
        root.copy_fixture(
            "presets/check-cargo.toml",
            ".toven/lang/rust/presets/check.toml",
        )
        .expect("copy project preset fixture");
        home.copy_fixture(
            "presets/check-user-cargo.toml",
            ".toven/lang/rust/presets/check.toml",
        )
        .expect("copy user preset fixture");

        let preset = PresetResolver::new(root.path().to_path_buf())
            .with_user_home(home.path().to_path_buf())
            .resolve("rust", "check")
            .expect("preset resolves");

        assert_eq!(preset.argv[0], "cargo");
    }

    #[test]
    fn rejects_invalid_preset_template() {
        let root = rskit_testutil::test_workspace!("invalid-template");
        root.copy_fixture(
            "presets/check-invalid-template.toml",
            ".toven/lang/rust/presets/check.toml",
        )
        .expect("copy preset fixture");

        let error = PresetResolver::new(root.path().to_path_buf())
            .without_user_home()
            .resolve("rust", "check")
            .expect_err("invalid template should fail");

        assert!(error.message.contains("unknown placeholder"));
    }
}
