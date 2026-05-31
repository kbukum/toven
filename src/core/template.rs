//! Toven template placeholders built on rskit-config template primitives.

use std::{fmt, path::Path};

use crate::core::{AppError, AppResult, Module, ScopeId};

const PLACEHOLDERS: &[Placeholder] = &[
    Placeholder::Args,
    Placeholder::ProjectRoot,
    Placeholder::WorkspaceRoot,
    Placeholder::ScopeRoot,
    Placeholder::ScopeId,
    Placeholder::ModuleScope,
    Placeholder::ModuleName,
    Placeholder::ModulePackage,
    Placeholder::ModulePath,
    Placeholder::ModuleManifest,
    Placeholder::ModuleArgs,
];

/// A supported placeholder.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Placeholder {
    /// User passthrough arguments.
    Args,
    /// Project root.
    ProjectRoot,
    /// Alias for project root kept for existing template vocabulary.
    WorkspaceRoot,
    /// Scope root relative to the project root.
    ScopeRoot,
    /// Scope identifier.
    ScopeId,
    /// Scope identifier that owns the module.
    ModuleScope,
    /// Module name.
    ModuleName,
    /// Module package name.
    ModulePackage,
    /// Module root path.
    ModulePath,
    /// Module manifest path.
    ModuleManifest,
    /// Repeated per-module selector arguments.
    ModuleArgs,
}

impl Placeholder {
    /// Return the user-facing template token for this placeholder.
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Args => "args",
            Self::ProjectRoot => "project.root",
            Self::WorkspaceRoot => "workspace.root",
            Self::ScopeRoot => "scope.root",
            Self::ScopeId => "scope.id",
            Self::ModuleScope => "module.scope",
            Self::ModuleName => "module.name",
            Self::ModulePackage => "module.package",
            Self::ModulePath => "module.path",
            Self::ModuleManifest => "module.manifest",
            Self::ModuleArgs => "module.args",
        }
    }
}

impl rskit_config::TemplatePlaceholder for Placeholder {
    fn token(self) -> &'static str {
        self.as_token()
    }
}

impl fmt::Display for Placeholder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_token())
    }
}

/// One parsed template part.
pub type TemplatePart = rskit_config::TemplatePart<Placeholder>;

/// Parsed template string.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Template {
    inner: rskit_config::Template<Placeholder>,
}

impl Template {
    /// Parse a template string and reject unknown placeholders.
    pub fn parse(value: &str) -> AppResult<Self> {
        Ok(Self {
            inner: rskit_config::Template::parse(value, PLACEHOLDERS)?,
        })
    }

    /// Return parsed template parts.
    #[must_use]
    pub fn parts(&self) -> &[TemplatePart] {
        self.inner.parts()
    }

    /// Return true when the template contains the placeholder.
    #[must_use]
    pub fn contains(&self, placeholder: Placeholder) -> bool {
        self.inner.contains(placeholder)
    }

    /// Render placeholders that produce a single scalar value.
    pub fn render_scalar(
        &self,
        workspace_root: &Path,
        module: Option<&Module>,
    ) -> AppResult<String> {
        self.render_scalar_with_scope(
            workspace_root,
            None,
            module.map(|module| &module.scope_id),
            module,
        )
    }

    /// Render placeholders that produce a single scalar value with scope context.
    pub fn render_scalar_with_scope(
        &self,
        project_root: &Path,
        scope_root: Option<&Path>,
        scope_id: Option<&ScopeId>,
        module: Option<&Module>,
    ) -> AppResult<String> {
        self.inner.render_with(|placeholder| {
            render_placeholder(placeholder, project_root, scope_root, scope_id, module)
        })
    }
}

fn render_placeholder(
    placeholder: Placeholder,
    project_root: &Path,
    scope_root: Option<&Path>,
    scope_id: Option<&ScopeId>,
    module: Option<&Module>,
) -> AppResult<String> {
    match placeholder {
        Placeholder::ProjectRoot | Placeholder::WorkspaceRoot => {
            Ok(project_root.display().to_string())
        }
        Placeholder::ScopeRoot => Ok(scope_root
            .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf)
            .display()
            .to_string()),
        Placeholder::ScopeId => scope_id
            .map(ToString::to_string)
            .or_else(|| module.map(|module| module.scope_id.to_string()))
            .ok_or_else(|| missing_scope(placeholder)),
        Placeholder::ModuleScope => module
            .map(|module| module.scope_id.to_string())
            .ok_or_else(|| missing_module(placeholder)),
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
        Placeholder::ModuleManifest => {
            let module = module.ok_or_else(|| missing_module(placeholder))?;
            module
                .manifest
                .as_ref()
                .map(|manifest| manifest.display().to_string())
                .ok_or_else(|| missing_manifest(placeholder, &module.name))
        }
        Placeholder::Args | Placeholder::ModuleArgs => Err(AppError::invalid_input(
            "template",
            "selector placeholders cannot be rendered as scalar values",
        )),
    }
}

fn missing_scope(placeholder: Placeholder) -> AppError {
    AppError::invalid_input(
        "template",
        format!("placeholder '{placeholder}' requires a scope"),
    )
}

fn missing_module(placeholder: Placeholder) -> AppError {
    AppError::invalid_input(
        "template",
        format!("placeholder '{placeholder}' requires a module"),
    )
}

fn missing_manifest(placeholder: Placeholder, module: &crate::core::ModuleId) -> AppError {
    AppError::invalid_input(
        "template",
        format!("placeholder '{placeholder}' requires module '{module}' to have a manifest"),
    )
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
    fn exposes_placeholder_tokens() {
        assert_eq!(Placeholder::Args.as_token(), "args");
        assert_eq!(Placeholder::ProjectRoot.to_string(), "project.root");
        assert_eq!(Placeholder::WorkspaceRoot.to_string(), "workspace.root");
        assert_eq!(Placeholder::ScopeRoot.to_string(), "scope.root");
        assert_eq!(Placeholder::ScopeId.to_string(), "scope.id");
        assert_eq!(Placeholder::ModuleScope.to_string(), "module.scope");
        assert_eq!(Placeholder::ModuleName.to_string(), "module.name");
        assert_eq!(Placeholder::ModulePackage.to_string(), "module.package");
        assert_eq!(Placeholder::ModulePath.to_string(), "module.path");
        assert_eq!(Placeholder::ModuleManifest.to_string(), "module.manifest");
        assert_eq!(Placeholder::ModuleArgs.to_string(), "module.args");
    }

    #[test]
    fn rejects_unknown_placeholders() {
        let error = Template::parse("{project.name}").expect_err("unknown placeholder fails");

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
    fn rejects_unmatched_closing_braces() {
        let error = Template::parse("cargo } {module.name}")
            .expect_err("unmatched closing brace should fail");

        assert!(error.message.contains("unmatched closing"));
    }

    #[test]
    fn renders_workspace_and_module_scalar_placeholders() {
        let module = crate::core::Module {
            scope_id: crate::core::ScopeId::new("rust").expect("scope id"),
            adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
            name: crate::core::ModuleId::new("api").expect("module id parses"),
            package: Some("api-pkg".to_string()),
            root: "crates/api".into(),
            manifest: Some("Cargo.toml".into()),
            dependencies: Vec::new(),
            source_patterns: Vec::new(),
        };
        let template = Template::parse(
            "{workspace.root}/{module.path}:{module.name}:{module.package}:{module.manifest}:{module.scope}",
        )
        .expect("template parses");

        let rendered = template
            .render_scalar(Path::new("/workspace"), Some(&module))
            .expect("template renders");

        assert_eq!(
            rendered,
            "/workspace/crates/api:api:api-pkg:Cargo.toml:rust"
        );
    }

    #[test]
    fn renders_project_and_scope_scalar_placeholders() {
        let template = Template::parse("{project.root}:{workspace.root}:{scope.root}:{scope.id}")
            .expect("template parses");

        let rendered = template
            .render_scalar_with_scope(
                Path::new("/workspace"),
                Some(Path::new("core")),
                Some(&crate::core::ScopeId::new("rust").expect("scope id")),
                None,
            )
            .expect("template renders");

        assert_eq!(rendered, "/workspace:/workspace:core:rust");
    }

    #[test]
    fn module_package_falls_back_to_module_name() {
        let module = crate::core::Module {
            scope_id: crate::core::ScopeId::new("rust").expect("scope id"),
            adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
            name: crate::core::ModuleId::new("api").expect("module id parses"),
            package: None,
            root: "crates/api".into(),
            manifest: Some("Cargo.toml".into()),
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
        assert!(error.message.contains("module.name"));
        assert!(!error.message.contains("ModuleName"));
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
