//! Command-backed discovery adapter.

use std::time::Duration;

use rskit_process::{InputPolicy, OutputPolicy, ProcessConfig, ProcessSpec};

use crate::core::{
    AdapterId, AppError, AppResult, DiscoverRequest, DiscoverResponse, DiscoveryAdapter,
    Placeholder, Template, TemplatePart, validate_discovery_request_schema,
    validate_discovery_response,
};

const DISCOVERY_COMMAND_TIMEOUT_SECS: u64 = 120;
const DISCOVERY_COMMAND_TIMEOUT: Duration = Duration::from_secs(DISCOVERY_COMMAND_TIMEOUT_SECS);
const DISCOVERY_COMMAND_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Adapter that delegates discovery to a user-provided process.
#[derive(Debug, Clone)]
pub struct CommandAdapter {
    adapter_id: AdapterId,
    argv: Vec<String>,
    field: String,
    config: ProcessConfig,
}

impl CommandAdapter {
    /// Create a command adapter.
    pub fn new(adapter_id: impl Into<String>, argv: Vec<String>) -> AppResult<Self> {
        Self::with_field(adapter_id, argv, "discovery_command")
    }

    /// Create a command adapter and preserve the config field path in errors.
    pub fn with_field(
        adapter_id: impl Into<String>,
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
            adapter_id: AdapterId::new(adapter_id.into())?,
            argv,
            field: field.to_string(),
            config: discovery_process_config(),
        })
    }

    fn render_command(&self, request: &DiscoverRequest) -> AppResult<(ProcessSpec, Vec<u8>)> {
        let rendered = render_argv(&self.argv, request)?;
        let mut iter = rendered.into_iter();
        let program = iter
            .next()
            .ok_or_else(|| AppError::invalid_input(&self.field, "missing program"))?;

        let stdin = serde_json::to_vec(request).map_err(AppError::internal)?;
        Ok((
            ProcessSpec::new(program)
                .args(iter)
                .dir(request.project_root.clone()),
            stdin,
        ))
    }
}

impl DiscoveryAdapter for CommandAdapter {
    fn adapter_id(&self) -> &AdapterId {
        &self.adapter_id
    }

    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        validate_discovery_request_schema(&self.field, request)?;

        let (command, stdin) = self.render_command(request)?;
        let config = self.config.clone().with_input(InputPolicy::Bytes(stdin));
        let result = rskit_process::run(&command, &config)?;
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
        validate_discovery_response(&self.field, request, &response)?;
        Ok(response)
    }
}

fn discovery_process_config() -> ProcessConfig {
    captured_config(
        Some(DISCOVERY_COMMAND_TIMEOUT),
        InputPolicy::Closed,
        OutputPolicy::captured().with_max_output_bytes(DISCOVERY_COMMAND_MAX_OUTPUT_BYTES),
    )
}

fn captured_config(
    timeout: Option<Duration>,
    input: InputPolicy,
    output: OutputPolicy,
) -> ProcessConfig {
    ProcessConfig::default()
        .with_timeout(timeout)
        .with_io(rskit_process::ProcessIo::captured(
            rskit_process::CapturedIo::new()
                .with_input(input)
                .with_output(output),
        ))
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
                && !matches!(
                    placeholder,
                    Placeholder::ProjectRoot
                        | Placeholder::WorkspaceRoot
                        | Placeholder::ScopeRoot
                        | Placeholder::ScopeId
                )
            {
                return Err(AppError::invalid_input(
                    field,
                    format!(
                        "discovery command only supports '{{{}}}', '{{{}}}', '{{{}}}', and '{{{}}}' placeholders",
                        Placeholder::ProjectRoot.as_token(),
                        Placeholder::WorkspaceRoot.as_token(),
                        Placeholder::ScopeRoot.as_token(),
                        Placeholder::ScopeId.as_token()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn render_argv(argv: &[String], request: &DiscoverRequest) -> AppResult<Vec<String>> {
    argv.iter()
        .map(|value| {
            Template::parse(value)?.render_scalar_with_scope(
                &request.project_root,
                Some(&request.scope_root),
                Some(&request.scope_id),
                None,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::core::{
        AdapterId, AdapterOptions, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoveryAdapter,
        ScopeId,
    };

    use super::CommandAdapter;

    fn request() -> DiscoverRequest {
        DiscoverRequest {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            project_root: std::env::current_dir().expect("current dir"),
            scope_id: ScopeId::new("custom").expect("scope id"),
            adapter_id: AdapterId::new("custom").expect("adapter id"),
            scope_root: PathBuf::from("."),
            adapter_options: AdapterOptions::default(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn parses_discovery_response_from_stdout() {
        let adapter = CommandAdapter::new(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!(
                    r#"cat >/dev/null; printf '\173"schema_version":{DISCOVERY_SCHEMA_VERSION},"scope_id":"custom","adapter_id":"custom","modules":[]\175'"#
                ),
            ],
        )
        .expect("adapter builds");

        let response = adapter.discover(&request()).expect("command discovers");

        assert_eq!(response.schema_version, DISCOVERY_SCHEMA_VERSION);
        assert!(response.modules.is_empty());
    }

    #[test]
    fn rejects_module_placeholders_in_discovery_command() {
        let error = CommandAdapter::new("custom", vec!["discover-{module.name}".to_string()])
            .expect_err("module placeholder should fail");

        assert!(error.message.contains("discovery_command"));
        assert!(error.message.contains("project.root"));
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
            "profiles.custom.discover",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&DiscoverRequest {
                schema_version: 0,
                ..request()
            })
            .expect_err("schema mismatch should fail before command execution");

        assert!(error.message.contains("profiles.custom.discover"));
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
            "profiles.custom.discover",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&request())
            .expect_err("invalid json should fail");

        assert!(error.message.contains("profiles.custom.discover"));
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
                r#"cat >/dev/null; printf '\173"schema_version":0,"scope_id":"custom","adapter_id":"custom","modules":[]\175'"#.to_string(),
            ],
            "profiles.custom.discover",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&request())
            .expect_err("schema mismatch should fail");

        assert!(error.message.contains("profiles.custom.discover"));
        assert!(
            error
                .message
                .contains("unsupported discovery response schema")
        );
    }

    #[test]
    #[cfg(unix)]
    fn rejects_scope_mismatch() {
        let adapter = CommandAdapter::with_field(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                r#"cat >/dev/null; printf '\173"schema_version":1,"scope_id":"other","adapter_id":"custom","modules":[]\175'"#.to_string(),
            ],
            "profiles.custom.discover",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&request())
            .expect_err("scope mismatch should fail");

        assert!(error.message.contains("response scope"));
    }

    #[test]
    #[cfg(unix)]
    fn rejects_module_scope_mismatch() {
        let adapter = CommandAdapter::with_field(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                r#"cat >/dev/null; printf '\173"schema_version":1,"scope_id":"custom","adapter_id":"custom","modules":[\173"scope_id":"other","adapter_id":"custom","name":"api","package":null,"root":".","dependencies":[],"source_patterns":[]\175]\175'"#.to_string(),
            ],
            "profiles.custom.discover",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&request())
            .expect_err("module scope mismatch should fail");

        assert!(error.message.contains("module 0 scope"));
    }

    #[test]
    #[cfg(unix)]
    fn rejects_module_adapter_mismatch() {
        let adapter = CommandAdapter::with_field(
            "custom",
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                r#"cat >/dev/null; printf '\173"schema_version":1,"scope_id":"custom","adapter_id":"custom","modules":[\173"scope_id":"custom","adapter_id":"other","name":"api","package":null,"root":".","dependencies":[],"source_patterns":[]\175]\175'"#.to_string(),
            ],
            "profiles.custom.discover",
        )
        .expect("adapter builds");

        let error = adapter
            .discover(&request())
            .expect_err("module adapter mismatch should fail");

        assert!(error.message.contains("module 0 adapter"));
    }
}
