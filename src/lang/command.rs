//! Command-backed language adapter.

use std::{path::Path, time::Duration};

use rskit_process::{Command, ProcessConfig};

use crate::core::{
    AppError, AppResult, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse, LangAdapter,
    Placeholder, Template, TemplatePart, validate_discovery_request_schema,
};

const DISCOVERY_COMMAND_TIMEOUT_SECS: u64 = 120;
const DISCOVERY_COMMAND_TIMEOUT: Duration = Duration::from_secs(DISCOVERY_COMMAND_TIMEOUT_SECS);
const DISCOVERY_COMMAND_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Language adapter that delegates discovery to a user-provided command.
#[derive(Debug, Clone)]
pub struct CommandAdapter {
    language: String,
    argv: Vec<String>,
    field: String,
    config: ProcessConfig,
}

impl CommandAdapter {
    /// Create a command adapter.
    pub fn new(language: impl Into<String>, argv: Vec<String>) -> AppResult<Self> {
        Self::with_field(language, argv, "discovery_command")
    }

    /// Create a command adapter and preserve the config field path in errors.
    pub fn with_field(
        language: impl Into<String>,
        argv: Vec<String>,
        field: impl AsRef<str>,
    ) -> AppResult<Self> {
        let field = field.as_ref();
        if argv.is_empty() {
            return Err(AppError::invalid_input(
                field,
                "at least one argv item is required",
            ));
        }
        validate_discovery_templates(field, &argv)?;

        Ok(Self {
            language: language.into(),
            argv,
            field: field.to_string(),
            config: discovery_process_config(),
        })
    }

    fn render_command(&self, request: &DiscoverRequest) -> AppResult<Command> {
        let rendered = render_argv(&self.argv, &request.workspace_root)?;
        let mut iter = rendered.into_iter();
        let program = iter
            .next()
            .ok_or_else(|| AppError::invalid_input(&self.field, "missing program"))?;

        let stdin = serde_json::to_vec(request).map_err(AppError::internal)?;
        Ok(Command::new(program)
            .args(iter)
            .dir(request.workspace_root.clone())
            .stdin(stdin))
    }
}

impl LangAdapter for CommandAdapter {
    fn language(&self) -> &str {
        &self.language
    }

    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        validate_discovery_request_schema(&self.field, request)?;

        let command = self.render_command(request)?;
        let result = rskit_process::run(&command, &self.config)?;
        result.check()?;

        if result.stdout_truncated {
            return Err(AppError::invalid_input(
                &self.field,
                "discovery command stdout exceeded capture limit",
            ));
        }
        if result.stderr_truncated {
            return Err(AppError::invalid_input(
                &self.field,
                "discovery command stderr exceeded capture limit",
            ));
        }

        let response: DiscoverResponse =
            serde_json::from_slice(&result.stdout_bytes).map_err(|error| {
                AppError::invalid_input(
                    &self.field,
                    format!("failed to parse discovery response JSON: {error}"),
                )
            })?;
        if response.schema_version != DISCOVERY_SCHEMA_VERSION {
            return Err(AppError::invalid_input(
                &self.field,
                format!(
                    "unsupported discovery response schema {}",
                    response.schema_version
                ),
            ));
        }
        Ok(response)
    }
}

const fn discovery_process_config() -> ProcessConfig {
    ProcessConfig {
        timeout: Some(DISCOVERY_COMMAND_TIMEOUT),
        grace_period: Duration::from_secs(5),
        capture_output: true,
        inherit_env: true,
        max_output_bytes: Some(DISCOVERY_COMMAND_MAX_OUTPUT_BYTES),
    }
}

fn validate_discovery_templates(field: &str, argv: &[String]) -> AppResult<()> {
    for value in argv {
        let template = Template::parse(value).map_err(|error| {
            AppError::invalid_input(
                field,
                format!("invalid template '{value}': {}", error.message),
            )
        })?;
        for part in template.parts() {
            if let TemplatePart::Placeholder(placeholder) = part
                && *placeholder != Placeholder::WorkspaceRoot
            {
                return Err(AppError::invalid_input(
                    field,
                    format!(
                        "discovery_command only supports '{{{}}}' placeholders",
                        Placeholder::WorkspaceRoot.as_token()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn render_argv(argv: &[String], workspace_root: &Path) -> AppResult<Vec<String>> {
    argv.iter()
        .map(|value| Template::parse(value)?.render_scalar(workspace_root, None))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::core::{DISCOVERY_SCHEMA_VERSION, DiscoverRequest, LangAdapter};

    use super::CommandAdapter;

    #[test]
    #[cfg(unix)]
    fn parses_discovery_response_from_stdout() {
        let adapter = CommandAdapter::new(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!(
                    r#"cat >/dev/null; printf '\173"schema_version":{DISCOVERY_SCHEMA_VERSION},"modules":[]\175'"#
                ),
            ],
        )
        .expect("adapter builds");

        let response = adapter
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                workspace_root: std::env::current_dir().expect("current dir"),
            })
            .expect("command discovers");

        assert_eq!(response.schema_version, DISCOVERY_SCHEMA_VERSION);
        assert!(response.modules.is_empty());
    }

    #[test]
    fn rejects_module_placeholders_in_discovery_command() {
        let error = CommandAdapter::new("custom", vec!["discover-{module.name}".to_string()])
            .expect_err("module placeholder should fail");

        assert!(error.message.contains("discovery_command"));
        assert!(error.message.contains("workspace.root"));
    }

    #[test]
    #[cfg(unix)]
    fn rejects_request_schema_mismatch_before_running_command() {
        let adapter = CommandAdapter::with_field(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "exit 99".to_string(),
            ],
            "profiles.custom.discovery_command",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&DiscoverRequest {
                schema_version: 0,
                workspace_root: std::env::current_dir().expect("current dir"),
            })
            .expect_err("schema mismatch should fail before command execution");

        assert!(error.message.contains("profiles.custom.discovery_command"));
        assert!(
            error
                .message
                .contains("unsupported discovery request schema")
        );
    }

    #[test]
    #[cfg(unix)]
    fn reports_invalid_discovery_json() {
        let adapter = CommandAdapter::with_field(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "cat >/dev/null; printf not-json".to_string(),
            ],
            "profiles.custom.discovery_command",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                workspace_root: std::env::current_dir().expect("current dir"),
            })
            .expect_err("invalid json should fail");

        assert!(error.message.contains("profiles.custom.discovery_command"));
        assert!(error.message.contains("JSON"));
    }

    #[test]
    #[cfg(unix)]
    fn reports_schema_mismatch_with_config_field() {
        let adapter = CommandAdapter::with_field(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                r#"cat >/dev/null; printf '\173"schema_version":0,"modules":[]\175'"#.to_string(),
            ],
            "profiles.custom.discovery_command",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                workspace_root: std::env::current_dir().expect("current dir"),
            })
            .expect_err("schema mismatch should fail");

        assert!(error.message.contains("profiles.custom.discovery_command"));
        assert!(
            error
                .message
                .contains("unsupported discovery response schema")
        );
    }
}
