//! Shared argv/resource template parser.

use std::path::Path;

use crate::core::{AppError, AppResult, Module};

/// A supported placeholder.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Placeholder {
    /// User passthrough arguments.
    Args,
    /// Workspace root.
    WorkspaceRoot,
    /// Module name.
    ModuleName,
    /// Module package name.
    ModulePackage,
    /// Module root path.
    ModulePath,
    /// Repeated per-module selector arguments.
    ModuleArgs,
}

/// One parsed template part.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TemplatePart {
    /// Literal text.
    Literal(String),
    /// Placeholder token.
    Placeholder(Placeholder),
}

/// Parsed template string.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Template {
    parts: Vec<TemplatePart>,
}

impl Template {
    /// Parse a template string and reject unknown placeholders.
    pub fn parse(value: &str) -> AppResult<Self> {
        let mut parts = Vec::new();
        let mut remaining = value;

        while let Some(start) = remaining.find('{') {
            if start > 0 {
                parts.push(TemplatePart::Literal(remaining[..start].to_string()));
            }
            let after_open = &remaining[start + 1..];
            let Some(end) = after_open.find('}') else {
                return Err(AppError::invalid_input(
                    "template",
                    format!("unclosed placeholder in '{value}'"),
                ));
            };
            let token = &after_open[..end];
            parts.push(TemplatePart::Placeholder(parse_placeholder(token)?));
            remaining = &after_open[end + 1..];
        }

        if !remaining.is_empty() {
            parts.push(TemplatePart::Literal(remaining.to_string()));
        }

        Ok(Self { parts })
    }

    /// Return parsed template parts.
    #[must_use]
    pub fn parts(&self) -> &[TemplatePart] {
        &self.parts
    }

    /// Return true when the template contains the placeholder.
    #[must_use]
    pub fn contains(&self, placeholder: Placeholder) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, TemplatePart::Placeholder(found) if *found == placeholder))
    }

    /// Render placeholders that produce a single scalar value.
    pub fn render_scalar(
        &self,
        workspace_root: &Path,
        module: Option<&Module>,
    ) -> AppResult<String> {
        let mut rendered = String::new();
        for part in &self.parts {
            match part {
                TemplatePart::Literal(value) => rendered.push_str(value),
                TemplatePart::Placeholder(placeholder) => {
                    rendered.push_str(&render_placeholder(*placeholder, workspace_root, module)?);
                }
            }
        }
        Ok(rendered)
    }
}

fn render_placeholder(
    placeholder: Placeholder,
    workspace_root: &Path,
    module: Option<&Module>,
) -> AppResult<String> {
    match placeholder {
        Placeholder::WorkspaceRoot => Ok(workspace_root.display().to_string()),
        Placeholder::ModuleName => module
            .map(|module| module.name.to_string())
            .ok_or_else(|| missing_module(placeholder)),
        Placeholder::ModulePackage => module
            .map(|module| {
                module
                    .package
                    .clone()
                    .unwrap_or_else(|| module.name.to_string())
            })
            .ok_or_else(|| missing_module(placeholder)),
        Placeholder::ModulePath => module
            .map(|module| module.root.display().to_string())
            .ok_or_else(|| missing_module(placeholder)),
        Placeholder::Args | Placeholder::ModuleArgs => Err(AppError::invalid_input(
            "template",
            "selector placeholders cannot be rendered as scalar values",
        )),
    }
}

fn missing_module(placeholder: Placeholder) -> AppError {
    AppError::invalid_input(
        "template",
        format!("placeholder '{placeholder:?}' requires a module"),
    )
}

fn parse_placeholder(token: &str) -> AppResult<Placeholder> {
    match token {
        "args" => Ok(Placeholder::Args),
        "workspace.root" => Ok(Placeholder::WorkspaceRoot),
        "module.name" => Ok(Placeholder::ModuleName),
        "module.package" => Ok(Placeholder::ModulePackage),
        "module.path" => Ok(Placeholder::ModulePath),
        "module.args" => Ok(Placeholder::ModuleArgs),
        "" => Err(AppError::invalid_input(
            "template",
            "placeholder cannot be empty",
        )),
        other => Err(AppError::invalid_input(
            "template",
            format!("unknown placeholder '{other}'"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Placeholder, Template, TemplatePart};

    #[test]
    fn parses_known_placeholders() {
        let template = Template::parse("cargo {module.name} {args}").expect("template parses");

        assert!(template.contains(Placeholder::ModuleName));
        assert!(template.contains(Placeholder::Args));
        assert_eq!(
            template.parts(),
            [
                TemplatePart::Literal("cargo ".to_string()),
                TemplatePart::Placeholder(Placeholder::ModuleName),
                TemplatePart::Literal(" ".to_string()),
                TemplatePart::Placeholder(Placeholder::Args),
            ]
        );
    }

    #[test]
    fn rejects_unknown_placeholders() {
        let error = Template::parse("{project.root}").expect_err("unknown placeholder fails");

        assert!(error.message.contains("unknown placeholder"));
    }

    #[test]
    fn rejects_unclosed_placeholders() {
        let error = Template::parse("cargo {module.name").expect_err("unclosed placeholder fails");

        assert!(error.message.contains("unclosed placeholder"));
    }

    #[test]
    fn rejects_empty_placeholders() {
        let error = Template::parse("{}").expect_err("empty placeholder fails");

        assert!(error.message.contains("placeholder cannot be empty"));
    }

    #[test]
    fn renders_workspace_and_module_scalar_placeholders() {
        let module = crate::core::Module {
            name: crate::core::ModuleId::new("api"),
            package: Some("api-pkg".to_string()),
            root: "crates/api".into(),
            dependencies: Vec::new(),
            source_patterns: Vec::new(),
        };
        let template =
            Template::parse("{workspace.root}/{module.path}:{module.name}:{module.package}")
                .expect("template parses");

        let rendered = template
            .render_scalar(Path::new("/workspace"), Some(&module))
            .expect("template renders");

        assert_eq!(rendered, "/workspace/crates/api:api:api-pkg");
    }

    #[test]
    fn module_package_falls_back_to_module_name() {
        let module = crate::core::Module {
            name: crate::core::ModuleId::new("api"),
            package: None,
            root: "crates/api".into(),
            dependencies: Vec::new(),
            source_patterns: Vec::new(),
        };
        let template = Template::parse("{module.package}").expect("template parses");

        let rendered = template
            .render_scalar(Path::new("/workspace"), Some(&module))
            .expect("template renders");

        assert_eq!(rendered, "api");
    }

    #[test]
    fn rejects_module_placeholder_without_module() {
        let error = Template::parse("{module.name}")
            .expect("template parses")
            .render_scalar(Path::new("/workspace"), None)
            .expect_err("module placeholder requires module");

        assert!(error.message.contains("requires a module"));
    }

    #[test]
    fn rejects_selector_placeholder_as_scalar() {
        let error = Template::parse("{args}")
            .expect("template parses")
            .render_scalar(Path::new("."), None)
            .expect_err("selector placeholder cannot render scalar");

        assert!(error.message.contains("selector placeholders"));
    }
}
