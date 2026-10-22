//! [`ReleaseTemplate`] — the one parser/renderer for release templates
//! (`tag_format`, `tag_message`, `commit_message`, image `name`/`tag`).

use rskit_errors::{AppError, AppResult};
use rskit_util::{Template, TemplatePart};
use rskit_version::semver::Version;
use toven_model::Module;

use super::ReleaseVar;
use crate::release::TagScheme;

/// A parsed release template bound to the config field it came from.
///
/// Every error names that field. Each caller states its `{channel}` rule by
/// what it passes to [`render`](Self::render): the tag/commit annotations pass
/// the entry's prerelease channel, an image passes `None` (stable). A tag
/// format goes through [`tag_scheme`](Self::tag_scheme) instead, which rejects
/// `{channel}` because the channel is already part of `{version}`.
#[derive(Debug, Clone)]
pub struct ReleaseTemplate {
    field: String,
    template: Template<ReleaseVar>,
}

impl ReleaseTemplate {
    /// Parse `text` against the [`ReleaseVar`] vocabulary.
    ///
    /// # Errors
    /// Rejects malformed syntax or an unknown placeholder, naming `field`.
    pub fn parse(field: impl Into<String>, text: &str) -> AppResult<Self> {
        let field = field.into();
        let template = Template::parse(text, ReleaseVar::ALL).map_err(|error| {
            AppError::invalid_input(&field, format!("invalid release template: {error}"))
                .with_cause(error)
        })?;
        Ok(Self { field, template })
    }

    /// Render for `module` at `version`; `channel` fills `{channel}` (`None`
    /// renders it empty).
    ///
    /// # Errors
    /// Fails only if the template references a placeholder this renderer does
    /// not know.
    pub fn render(
        &self,
        module: &Module,
        version: &Version,
        channel: Option<&str>,
    ) -> AppResult<String> {
        self.template
            .render_with(|placeholder| match placeholder {
                ReleaseVar::Version => Ok(version.to_string()),
                ReleaseVar::Channel => Ok(channel.unwrap_or_default().to_string()),
                other => module_value(other, module),
            })
            .map_err(|error| {
                AppError::invalid_input(
                    &self.field,
                    format!("failed to render release template: {error}"),
                )
                .with_cause(error)
            })
    }

    /// Check the tag-format shape: exactly one `{version}` and no `{channel}`.
    ///
    /// # Errors
    /// Rejects either shape violation, naming the field.
    pub fn validate_tag_shape(&self) -> AppResult<()> {
        let versions = self
            .template
            .parts()
            .iter()
            .filter(|part| matches!(part, TemplatePart::Placeholder(ReleaseVar::Version)))
            .count();
        if versions != 1 {
            return Err(AppError::invalid_input(
                &self.field,
                "release tag template must contain exactly one {version} placeholder",
            ));
        }
        if self.template.contains(ReleaseVar::Channel) {
            return Err(AppError::invalid_input(
                &self.field,
                "release tag template must not contain {channel}: the prerelease channel is part of {version}",
            ));
        }
        Ok(())
    }

    /// Build `module`'s [`TagScheme`]: the parts before `{version}` form the
    /// prefix and the parts after it the suffix.
    ///
    /// # Errors
    /// Rejects a template that fails [`validate_tag_shape`](Self::validate_tag_shape).
    pub fn tag_scheme(&self, module: &Module) -> AppResult<TagScheme> {
        self.validate_tag_shape()?;
        let mut prefix = String::new();
        let mut suffix = String::new();
        let mut seen_version = false;
        for part in self.template.parts() {
            let text = match part {
                TemplatePart::Placeholder(ReleaseVar::Version) => {
                    seen_version = true;
                    continue;
                }
                TemplatePart::Placeholder(placeholder) => module_value(*placeholder, module)?,
                TemplatePart::Literal(literal) => literal.clone(),
            };
            if seen_version {
                suffix.push_str(&text);
            } else {
                prefix.push_str(&text);
            }
        }
        Ok(TagScheme::new(prefix, suffix))
    }
}

fn module_value(placeholder: ReleaseVar, module: &Module) -> AppResult<String> {
    match placeholder {
        ReleaseVar::Ecosystem => Ok(module.id.ecosystem.as_str().to_string()),
        ReleaseVar::Module => Ok(module.id.name.clone()),
        other => Err(AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("release template placeholder {{{other}}} has no module value"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};

    use super::ReleaseTemplate;
    use crate::release::TagScheme;

    fn module() -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
            RepoPath::new("crates/core").unwrap(),
        )
    }

    fn version() -> Version {
        Version::parse("1.2.0-rc.1").unwrap()
    }

    #[test]
    fn parse_names_the_field_on_an_unknown_placeholder() {
        let error = ReleaseTemplate::parse("release.tag_format", "v{verison}").unwrap_err();
        assert!(error.to_string().contains("release.tag_format"));
    }

    #[test]
    fn render_fills_every_placeholder_and_the_given_channel() {
        let template =
            ReleaseTemplate::parse("f", "{ecosystem}/{module}@{version} ({channel})").unwrap();
        assert_eq!(
            template.render(&module(), &version(), Some("rc")).unwrap(),
            "rust/core@1.2.0-rc.1 (rc)"
        );
        assert_eq!(
            template.render(&module(), &version(), None).unwrap(),
            "rust/core@1.2.0-rc.1 ()"
        );
    }

    #[test]
    fn tag_scheme_splits_at_version_and_renders_module_parts() {
        let template = ReleaseTemplate::parse("f", "{ecosystem}/{module}@{version}-x").unwrap();
        assert_eq!(
            template.tag_scheme(&module()).unwrap(),
            TagScheme::new("rust/core@", "-x")
        );
    }

    #[test]
    fn tag_scheme_rejects_zero_or_two_versions_and_channel() {
        for text in ["v", "{version}-{version}", "{channel}/v{version}"] {
            let template = ReleaseTemplate::parse("release.tag_format", text).unwrap();
            let error = template.tag_scheme(&module()).unwrap_err();
            assert!(error.to_string().contains("release.tag_format"), "{text}");
        }
    }
}
