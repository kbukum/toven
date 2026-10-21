//! Project health checks (`[doctor.checks.<id>]`) that `toven doctor` runs
//! beside its tool audit.
//!
//! A check is a plain predicate: a project-relative `path` that must exist, or
//! an `argv` command that must succeed (and, with `version`, print that text).
//! `profiles` limits a check to the runs that select one of them, so a repo can
//! keep Docker-only checks out of the everyday audit.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::validate_safe_path;
use serde::{Deserialize, Serialize};

/// The `[doctor]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorConfig {
    /// Health checks keyed by id (`[doctor.checks.<id>]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, DoctorCheckConfig>,
}

/// One `[doctor.checks.<id>]` predicate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorCheckConfig {
    /// A command (argv, no shell) that must exit 0. Runs in the project root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    /// A project-relative path that must exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Text the `argv` output must contain, such as a pinned version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What to run or do when the check fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// Profiles that enable the check; empty means it always runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
}

impl DoctorConfig {
    /// Every profile any check declares.
    #[must_use]
    pub fn profiles(&self) -> BTreeSet<&str> {
        self.checks
            .values()
            .flat_map(|check| check.profiles.iter().map(String::as_str))
            .collect()
    }

    /// The checks a run with `selected` profiles performs: every check without
    /// profiles, plus those sharing a selected profile.
    ///
    /// # Errors
    /// Returns [`AppError::invalid_input`] for a selected profile no check
    /// declares, so a typo cannot silently skip checks.
    pub fn selected(&self, selected: &[String]) -> AppResult<Vec<(&str, &DoctorCheckConfig)>> {
        let known = self.profiles();
        if let Some(unknown) = selected.iter().find(|name| !known.contains(name.as_str())) {
            return Err(AppError::invalid_input(
                "profile",
                format!("no doctor check uses profile '{unknown}'"),
            ));
        }
        Ok(self
            .checks
            .iter()
            .filter(|(_, check)| {
                check.profiles.is_empty()
                    || check
                        .profiles
                        .iter()
                        .any(|profile| selected.contains(profile))
            })
            .map(|(id, check)| (id.as_str(), check))
            .collect())
    }
}

/// Validate every `[doctor.checks.<id>]`.
pub(super) fn validate_doctor(doctor: &DoctorConfig) -> AppResult<()> {
    for (id, check) in &doctor.checks {
        let field = format!("doctor.checks.{id}");
        validate_name(&field, id)?;
        match (check.argv.is_empty(), &check.path) {
            (false, None) => {}
            (true, Some(path)) => validate_safe_path(path).map_err(|error| {
                AppError::invalid_input(format!("{field}.path"), error.to_string())
                    .with_cause(error)
            })?,
            _ => {
                return Err(AppError::invalid_input(
                    field,
                    "set exactly one of `argv` or `path`",
                ));
            }
        }
        if check.argv.iter().any(|arg| arg.trim().is_empty()) {
            return Err(AppError::invalid_input(
                format!("{field}.argv"),
                "argv entries cannot be empty",
            ));
        }
        if check.version.is_some() && check.argv.is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.version"),
                "`version` needs an `argv` command to read it from",
            ));
        }
        for profile in &check.profiles {
            validate_name(&format!("{field}.profiles"), profile)?;
        }
    }
    Ok(())
}

/// Check ids and profiles are typed on the command line and printed in
/// reports, so they stay plain words: ASCII letters, digits, `-` and `_`.
fn validate_name(field: &str, name: &str) -> AppResult<()> {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if plain {
        Ok(())
    } else {
        Err(AppError::invalid_input(
            field,
            format!("`{name}` must use only ASCII letters, digits, `-` and `_`"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{DoctorCheckConfig, DoctorConfig, validate_doctor};

    fn config(entries: Vec<(&str, DoctorCheckConfig)>) -> DoctorConfig {
        DoctorConfig {
            checks: entries
                .into_iter()
                .map(|(id, check)| (id.to_string(), check))
                .collect(),
        }
    }

    fn argv(args: &[&str]) -> DoctorCheckConfig {
        DoctorCheckConfig {
            argv: args.iter().map(ToString::to_string).collect(),
            ..DoctorCheckConfig::default()
        }
    }

    fn path(path: &str) -> DoctorCheckConfig {
        DoctorCheckConfig {
            path: Some(path.to_string()),
            ..DoctorCheckConfig::default()
        }
    }

    #[test]
    fn valid_checks_pass() {
        let mut go = argv(&["go", "version"]);
        go.version = Some("go1.26".to_string());
        go.profiles = vec!["app".to_string()];
        validate_doctor(&config(vec![("go", go), ("pack", path("target/x"))])).unwrap();
    }

    #[test]
    fn malformed_checks_are_rejected() {
        assert!(validate_doctor(&config(vec![("x", DoctorCheckConfig::default())])).is_err());
        let mut both = argv(&["go"]);
        both.path = Some("a".to_string());
        assert!(validate_doctor(&config(vec![("x", both)])).is_err());
        assert!(validate_doctor(&config(vec![("x", path("../a"))])).is_err());
        let mut version = path("a");
        version.version = Some("1".to_string());
        assert!(validate_doctor(&config(vec![("x", version)])).is_err());
        assert!(validate_doctor(&config(vec![("x", argv(&["go", " "]))])).is_err());
        assert!(validate_doctor(&config(vec![("a b", argv(&["go"]))])).is_err());
    }

    #[test]
    fn profiles_select_checks_and_reject_unknown_names() {
        let mut docker = argv(&["docker", "info"]);
        docker.profiles = vec!["app".to_string()];
        let doctor = config(vec![("docker", docker), ("go", argv(&["go"]))]);
        let ids = |selected: &[String]| -> Vec<String> {
            doctor
                .selected(selected)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id.to_string())
                .collect()
        };
        assert_eq!(ids(&[]), ["go"]);
        assert_eq!(ids(&["app".to_string()]), ["docker", "go"]);
        assert!(doctor.selected(&["nope".to_string()]).is_err());
    }
}
