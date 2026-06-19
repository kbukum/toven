//! `CommandTemplate` — the two-template argv renderer (base + selector/args splices).

use rskit_errors::{AppError, AppResult};
use rskit_util::{Template, TemplatePart};

use super::TaskVar;

/// A task command as two parsed templates: the invariant `base` argv and the
/// per-module `selector` fragment, plus two list-valued splice points.
///
/// Both halves are vectors of rskit-util [`Template`]s — one per argv element —
/// so a standalone splice element (`{module.selector}` or `{args}`) expands to
/// **zero or more** argv entries while every other element renders to exactly
/// one. `{module.selector}` splices the rendered per-module selector fragment;
/// `{args}` splices the user passthrough tail **verbatim** (raw CLI tokens are
/// never re-templated, so an arg containing `{` is passed through untouched).
/// This is the "splice N times = batch, spawn-each = run N times, same two
/// templates" model.
#[derive(Debug, Clone)]
pub struct CommandTemplate {
    base: Vec<Template<TaskVar>>,
    selector: Vec<Template<TaskVar>>,
}

/// Placeholders that are list-valued splice points: legal only as a standalone
/// base argv element, never inside a literal or the selector fragment.
const SPLICE_VARS: [TaskVar; 2] = [TaskVar::ModuleSelector, TaskVar::Args];

impl CommandTemplate {
    /// Parse a base argv and selector fragment into typed templates.
    ///
    /// # Errors
    /// Rejects unknown placeholders (rskit-util strict parse), a splice token
    /// (`{module.selector}` or `{args}`) used anywhere but as a standalone base
    /// argv element, and any splice token inside the selector fragment.
    pub fn parse(base_argv: &[String], selector: &[String]) -> AppResult<Self> {
        let base = base_argv
            .iter()
            .map(|element| parse_base_element(element))
            .collect::<AppResult<Vec<_>>>()?;
        let selector = selector
            .iter()
            .map(|element| parse_selector_element(element))
            .collect::<AppResult<Vec<_>>>()?;
        Ok(Self { base, selector })
    }

    /// Render the command for one module, splicing the rendered selector and the
    /// `passthrough` args at their respective standalone elements.
    ///
    /// `passthrough` args are spliced verbatim at `{args}`; `resolve` supplies a
    /// value for every other placeholder except the splice tokens
    /// ([`TaskVar::ModuleSelector`], [`TaskVar::Args`]).
    ///
    /// # Errors
    /// Propagates any error returned by `resolve`.
    pub fn render<F>(&self, passthrough: &[String], mut resolve: F) -> AppResult<Vec<String>>
    where
        F: FnMut(TaskVar) -> AppResult<String>,
    {
        let selector = self
            .selector
            .iter()
            .map(|fragment| render_template(fragment, &mut resolve))
            .collect::<AppResult<Vec<_>>>()?;

        let mut argv = Vec::with_capacity(self.base.len() + selector.len() + passthrough.len());
        for element in &self.base {
            if is_splice(element, TaskVar::ModuleSelector) {
                argv.extend(selector.iter().cloned());
            } else if is_splice(element, TaskVar::Args) {
                argv.extend(passthrough.iter().cloned());
            } else {
                argv.push(render_template(element, &mut resolve)?);
            }
        }
        Ok(argv)
    }
}

fn parse_base_element(element: &str) -> AppResult<Template<TaskVar>> {
    let template = Template::parse(element, TaskVar::ALL).map_err(|error| to_app_error(&error))?;
    for var in SPLICE_VARS {
        if template.contains(var) && !is_splice(&template, var) {
            return Err(AppError::invalid_input(
                "task.argv",
                format!("'{{{var}}}' must be a standalone argv element, got '{element}'"),
            ));
        }
    }
    Ok(template)
}

fn parse_selector_element(element: &str) -> AppResult<Template<TaskVar>> {
    let template = Template::parse(element, TaskVar::ALL).map_err(|error| to_app_error(&error))?;
    for var in SPLICE_VARS {
        if template.contains(var) {
            return Err(AppError::invalid_input(
                "task.selector",
                format!("'{{{var}}}' cannot appear inside the selector fragment"),
            ));
        }
    }
    Ok(template)
}

/// True when the template is exactly the standalone `var` placeholder token.
fn is_splice(template: &Template<TaskVar>, var: TaskVar) -> bool {
    matches!(template.parts(), [TemplatePart::Placeholder(p)] if *p == var)
}

fn render_template<F>(template: &Template<TaskVar>, resolve: &mut F) -> AppResult<String>
where
    F: FnMut(TaskVar) -> AppResult<String>,
{
    template
        .render_with(&mut *resolve)
        .map_err(|error| to_app_error(&error))
}

fn to_app_error(error: &rskit_util::template::TemplateError) -> AppError {
    AppError::invalid_input("task.template", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{CommandTemplate, TaskVar};
    use rskit_util::Placeholder;

    fn resolve(value: TaskVar) -> String {
        match value {
            TaskVar::ModulePackage => "errors".to_string(),
            TaskVar::ModuleManifest => "core/Cargo.toml".to_string(),
            other => other.token().to_string(),
        }
    }

    #[test]
    fn rejects_unknown_placeholder() {
        let error = CommandTemplate::parse(&["{module.bogus}".to_string()], &[])
            .expect_err("unknown placeholder must be rejected");
        assert!(error.to_string().contains("module.bogus"), "{error}");
    }

    #[test]
    fn renders_two_template_with_selector_and_args_splice() {
        let base = [
            "cargo".to_string(),
            "test".to_string(),
            "--manifest-path".to_string(),
            "{module.manifest}".to_string(),
            "{module.selector}".to_string(),
            "{args}".to_string(),
        ];
        let selector = ["-p".to_string(), "{module.package}".to_string()];
        let passthrough = ["--nocapture".to_string(), "--test-threads=1".to_string()];

        let command = CommandTemplate::parse(&base, &selector).expect("templates parse");
        let argv = command
            .render(&passthrough, |var| Ok(resolve(var)))
            .expect("renders");

        assert_eq!(
            argv,
            vec![
                "cargo",
                "test",
                "--manifest-path",
                "core/Cargo.toml",
                "-p",
                "errors",
                "--nocapture",
                "--test-threads=1",
            ]
        );
    }

    #[test]
    fn empty_args_splice_to_zero_argv_elements() {
        let base = [
            "cargo".to_string(),
            "build".to_string(),
            "{args}".to_string(),
        ];

        let command = CommandTemplate::parse(&base, &[]).expect("templates parse");
        let argv = command
            .render(&[], |var| Ok(resolve(var)))
            .expect("renders");

        assert_eq!(argv, vec!["cargo", "build"]);
    }

    #[test]
    fn args_are_spliced_verbatim_not_re_templated() {
        let base = ["echo".to_string(), "{args}".to_string()];
        // A passthrough token containing template syntax must pass through untouched.
        let passthrough = ["{module.bogus}".to_string()];

        let command = CommandTemplate::parse(&base, &[]).expect("templates parse");
        let argv = command
            .render(&passthrough, |var| Ok(resolve(var)))
            .expect("renders");

        assert_eq!(argv, vec!["echo", "{module.bogus}"]);
    }

    #[test]
    fn rejects_selector_token_inside_literal() {
        let error = CommandTemplate::parse(&["x{module.selector}".to_string()], &[])
            .expect_err("inline selector must be rejected");
        assert!(error.to_string().contains("standalone"), "{error}");
    }

    #[test]
    fn rejects_args_token_inside_literal() {
        let error = CommandTemplate::parse(&["x{args}".to_string()], &[])
            .expect_err("inline args must be rejected");
        assert!(error.to_string().contains("standalone"), "{error}");
    }

    #[test]
    fn rejects_selector_token_in_selector_fragment() {
        let error = CommandTemplate::parse(&[], &["{module.selector}".to_string()])
            .expect_err("selector token in selector must be rejected");
        assert!(error.to_string().contains("selector fragment"), "{error}");
    }

    #[test]
    fn rejects_args_token_in_selector_fragment() {
        let error = CommandTemplate::parse(&[], &["{args}".to_string()])
            .expect_err("args token in selector must be rejected");
        assert!(error.to_string().contains("selector fragment"), "{error}");
    }
}
