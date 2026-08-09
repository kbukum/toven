//! Version-reference vocabulary: declare files whose embedded version tokens
//! `release bump` keeps in lock-step with the authoritative post-bump versions.
//!
//! A [`VersionReferenceConfig`] pairs a set of repo-relative file globs with a
//! per-line `pattern` — a [`Template`] over the `{module}` and `{version}`
//! placeholders (e.g. `{module} = "{version}"`). During the `bump` mutation the
//! engine rewrites only the `{version}` token of each pattern-matching line to
//! the authoritative version of the captured `{module}`, leaving prose and
//! examples that do not match the pattern untouched.

use std::fmt;

use rskit_errors::{AppError, AppResult};
use rskit_util::template::{Placeholder, Template, TemplatePart};
use serde::{Deserialize, Serialize};

/// A declared version-reference: files whose version tokens `release bump`
/// rewrites to the authoritative post-bump versions, inside the bump mutation
/// and staged with the manifests.
///
/// The rewrite is native (no shell), format-preserving (only the matched
/// `{version}` token changes), and idempotent (a line already at the
/// authoritative version is left byte-for-byte unchanged).
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionReferenceConfig {
    /// Repo-relative file globs (`*`/`?`) whose matching lines carry version
    /// pins (e.g. `README.md`, `crates/*/README.md`).
    pub files: Vec<String>,
    /// The per-line pin pattern: a template over `{module}` and `{version}`
    /// (e.g. `{module} = "{version}"`). A line is rewritten only when it matches
    /// the whole pattern (after leading whitespace) and the captured `{module}`
    /// resolves to a bumped version.
    pub pattern: String,
}

/// Placeholder tokens recognized in a version-reference [`pattern`](VersionReferenceConfig::pattern).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VersionRefToken {
    /// The module identifier a pin references (matched against a module's
    /// package name or `ecosystem:name` identity).
    Module,
    /// The semver version token the engine rewrites.
    Version,
}

impl Placeholder for VersionRefToken {
    fn token(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Version => "version",
        }
    }
}

impl fmt::Display for VersionRefToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

/// The placeholder set a version-reference pattern is parsed against.
pub const VERSION_REF_TOKENS: &[VersionRefToken] =
    &[VersionRefToken::Module, VersionRefToken::Version];

impl VersionReferenceConfig {
    /// Parse the [`pattern`](Self::pattern) into a typed [`Template`], rejecting
    /// unknown placeholders.
    ///
    /// # Errors
    /// Rejects a pattern that fails to parse, that omits either the `{module}`
    /// or `{version}` placeholder, or that places two placeholders adjacent with
    /// no literal separator between them (which would make the capture
    /// boundary ambiguous).
    pub fn parse_pattern(&self) -> AppResult<Template<VersionRefToken>> {
        parse_pattern("release.version_references.pattern", &self.pattern)
    }

    /// Validate every field beyond serde's type checks.
    ///
    /// `field` is the config path prefix used in diagnostics (e.g.
    /// `ecosystems.rust.release.version_references[0]`).
    ///
    /// # Errors
    /// Rejects an empty file list, a blank glob, and a malformed pattern.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if self.files.is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.files"),
                "a version reference must declare at least one file glob",
            ));
        }
        for (index, glob) in self.files.iter().enumerate() {
            if glob.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.files[{index}]"),
                    "must not be blank",
                ));
            }
        }
        parse_pattern(&format!("{field}.pattern"), &self.pattern)?;
        Ok(())
    }
}

/// Parse and structurally validate a version-reference pattern under `field`.
fn parse_pattern(field: &str, pattern: &str) -> AppResult<Template<VersionRefToken>> {
    if pattern.trim().is_empty() {
        return Err(AppError::invalid_input(field, "must not be blank"));
    }
    let template = Template::parse(pattern, VERSION_REF_TOKENS).map_err(|error| {
        AppError::invalid_input(field, format!("invalid version-reference pattern: {error}"))
            .with_cause(error)
    })?;
    if !template.contains(VersionRefToken::Module) || !template.contains(VersionRefToken::Version) {
        return Err(AppError::invalid_input(
            field,
            "a version-reference pattern must contain both the {module} and {version} \
             placeholders",
        ));
    }
    let mut previous_was_placeholder = false;
    for part in template.parts() {
        match part {
            TemplatePart::Placeholder(_) => {
                if previous_was_placeholder {
                    return Err(AppError::invalid_input(
                        field,
                        "a version-reference pattern cannot place two placeholders adjacent; \
                         separate {module} and {version} with literal text",
                    ));
                }
                previous_was_placeholder = true;
            }
            TemplatePart::Literal(_) => previous_was_placeholder = false,
        }
    }
    Ok(template)
}

#[cfg(test)]
mod tests {
    use super::{VersionRefToken, VersionReferenceConfig};

    fn config(pattern: &str) -> VersionReferenceConfig {
        VersionReferenceConfig {
            files: vec!["README.md".to_string()],
            pattern: pattern.to_string(),
        }
    }

    #[test]
    fn a_well_formed_pattern_parses_with_both_placeholders() {
        let template = config("{module} = \"{version}\"")
            .parse_pattern()
            .expect("parses");
        assert!(template.contains(VersionRefToken::Module));
        assert!(template.contains(VersionRefToken::Version));
    }

    #[test]
    fn validation_accepts_a_well_formed_reference() {
        config("{module} = \"{version}\"")
            .validate("release.version_references[0]")
            .expect("valid");
    }

    #[test]
    fn an_empty_file_list_is_rejected() {
        let reference = VersionReferenceConfig {
            files: Vec::new(),
            pattern: "{module} = \"{version}\"".to_string(),
        };
        let error = reference
            .validate("release.version_references[0]")
            .expect_err("empty file list rejected");
        assert!(error.to_string().contains("at least one file"), "{error}");
    }

    #[test]
    fn a_blank_glob_is_rejected() {
        let reference = VersionReferenceConfig {
            files: vec!["  ".to_string()],
            pattern: "{module} = \"{version}\"".to_string(),
        };
        let error = reference
            .validate("release.version_references[0]")
            .expect_err("blank glob rejected");
        assert!(error.to_string().contains("blank"), "{error}");
    }

    #[test]
    fn a_pattern_missing_the_version_placeholder_is_rejected() {
        let error = config("{module} pinned")
            .validate("release.version_references[0]")
            .expect_err("missing version rejected");
        assert!(error.to_string().contains("{version}"), "{error}");
    }

    #[test]
    fn a_pattern_with_an_unknown_placeholder_is_rejected() {
        let error = config("{crate} = \"{version}\"")
            .validate("release.version_references[0]")
            .expect_err("unknown placeholder rejected");
        assert!(error.to_string().contains("pattern"), "{error}");
    }

    #[test]
    fn adjacent_placeholders_are_rejected() {
        let error = config("{module}{version}")
            .validate("release.version_references[0]")
            .expect_err("adjacent placeholders rejected");
        assert!(error.to_string().contains("adjacent"), "{error}");
    }
}
