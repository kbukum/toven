//! [`PackCommand`] — a repository's own release pack command
//! (`[ecosystems.js] pack`), validated and rendered argv-first.

use std::fmt;
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_util::{Placeholder, Template, TemplatePart};

/// The placeholders a pack command may reference.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum PackVar {
    /// `{module.root}` — the repo-relative package directory (`.` for the
    /// repository root).
    ModuleRoot,
    /// `{destination}` — the absolute, empty directory the command must write
    /// exactly one tarball into.
    Destination,
}

impl PackVar {
    /// Every placeholder, for [`Template::parse`].
    const ALL: &'static [Self] = &[Self::ModuleRoot, Self::Destination];
}

impl Placeholder for PackVar {
    fn token(self) -> &'static str {
        match self {
            Self::ModuleRoot => "module.root",
            Self::Destination => "destination",
        }
    }
}

impl fmt::Display for PackVar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

/// A parsed pack command: a literal program followed by argument templates.
#[derive(Debug)]
pub(crate) struct PackCommand {
    program: String,
    args: Vec<Template<PackVar>>,
}

impl PackCommand {
    /// Parse and validate the authored argv for diagnostics under `field`.
    ///
    /// # Errors
    /// Rejects an empty argv, a blank or templated program, a blank argument,
    /// an unknown placeholder, and a command that never names
    /// `{destination}` (its tarball would land where Toven cannot find it).
    pub(crate) fn parse(field: &str, authored: &[String]) -> AppResult<Self> {
        let Some((program, rest)) = authored.split_first() else {
            return Err(AppError::invalid_input(
                field,
                "the pack command must not be empty",
            ));
        };
        if program.trim().is_empty() || program.contains(['{', '}']) {
            return Err(AppError::invalid_input(
                format!("{field}[0]"),
                "the pack program must be a literal, non-blank executable",
            ));
        }
        let mut args = Vec::with_capacity(rest.len());
        for (index, arg) in rest.iter().enumerate() {
            let at = format!("{field}[{}]", index + 1);
            if arg.trim().is_empty() {
                return Err(AppError::invalid_input(at, "argument must not be blank"));
            }
            let template = Template::parse(arg, PackVar::ALL).map_err(|error| {
                AppError::invalid_input(
                    at.clone(),
                    format!(
                        "invalid pack argument (placeholders: {{module.root}}, \
                         {{destination}}): {error}"
                    ),
                )
                .with_cause(error)
            })?;
            args.push(template);
        }
        if !args
            .iter()
            .any(|template| template.contains(PackVar::Destination))
        {
            return Err(AppError::invalid_input(
                field,
                "the pack command must pass {destination}, the directory Toven collects \
                 the tarball from",
            ));
        }
        Ok(Self {
            program: program.clone(),
            args,
        })
    }

    /// The executable the command runs, for diagnostics.
    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    /// Render the argv for one package.
    ///
    /// # Errors
    /// Fails when `destination` is not valid UTF-8.
    pub(crate) fn render(&self, module_root: &str, destination: &Path) -> AppResult<Vec<String>> {
        let destination = destination.to_str().ok_or_else(|| {
            AppError::invalid_input(
                "release.pack",
                format!(
                    "pack destination '{}' is not valid UTF-8",
                    destination.display()
                ),
            )
        })?;
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        for template in &self.args {
            let mut rendered = String::new();
            for part in template.parts() {
                match part {
                    TemplatePart::Literal(text) => rendered.push_str(text),
                    TemplatePart::Placeholder(PackVar::ModuleRoot) => {
                        rendered.push_str(module_root);
                    }
                    TemplatePart::Placeholder(PackVar::Destination) => {
                        rendered.push_str(destination);
                    }
                }
            }
            argv.push(rendered);
        }
        Ok(argv)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rskit_errors::ErrorCode;

    use super::PackCommand;

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn renders_module_root_and_destination_inside_arguments() {
        let command = PackCommand::parse(
            "ecosystems.js.pack.\".\"",
            &argv(&["bun", "pack.ts", "{module.root}", "--out={destination}"]),
        )
        .expect("parses");
        assert_eq!(command.program(), "bun");
        assert_eq!(
            command
                .render("packages/std", Path::new("/tmp/out"))
                .expect("renders"),
            argv(&["bun", "pack.ts", "packages/std", "--out=/tmp/out"])
        );
    }

    #[test]
    fn rejects_malformed_commands() {
        for (case, authored) in [
            ("empty", argv(&[])),
            ("blank program", argv(&[" ", "{destination}"])),
            (
                "templated program",
                argv(&["{module.root}", "{destination}"]),
            ),
            ("blank argument", argv(&["bun", "", "{destination}"])),
            (
                "unknown placeholder",
                argv(&["bun", "{module.name}", "{destination}"]),
            ),
            ("unclosed placeholder", argv(&["bun", "{destination"])),
            ("no destination", argv(&["bun", "pack", "{module.root}"])),
        ] {
            let error = PackCommand::parse("ecosystems.js.pack.web", &authored).expect_err(case);
            assert_eq!(error.code(), ErrorCode::InvalidInput, "{case}");
            assert!(
                error.to_string().contains("ecosystems.js.pack.web"),
                "{case}: {error}"
            );
        }
    }
}
