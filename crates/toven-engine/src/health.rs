//! Project health checks: evaluate `[doctor.checks.<id>]` predicates.
//!
//! Each check is a path that must exist or an argv command that must succeed
//! (and, with `version`, print that text). Commands run through the injected
//! [`ToolRunner`] with a bounded timeout and output cap, never a shell. This
//! module classifies outcomes; the CLI reports them.

use std::time::Duration;

use rskit_errors::AppResult;
use toven_core::config::{DoctorCheckConfig, DoctorConfig};
use toven_model::AbsPath;
use toven_ports::{ToolInvocation, ToolRunner};

/// How long one check command may run.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Output kept from one check command (enough for any version banner).
const CHECK_OUTPUT_BYTES: usize = 64 * 1024;

/// One evaluated health check.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckOutcome {
    /// The check id (`[doctor.checks.<id>]`).
    pub id: String,
    /// Why the check failed; `None` when it passed.
    pub failure: Option<String>,
    /// The configured fix hint.
    pub fix: Option<String>,
}

impl CheckOutcome {
    /// Whether the check passed.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.failure.is_none()
    }
}

/// Run the checks `profiles` selects, reporting each as it completes.
///
/// # Errors
/// Returns a typed error for an unknown profile, or propagates a failure from
/// `on_check`. A failing check is an outcome, not an error.
pub fn run_checks(
    project_root: &AbsPath,
    doctor: &DoctorConfig,
    profiles: &[String],
    runner: &dyn ToolRunner,
    on_check: &mut dyn FnMut(&CheckOutcome) -> AppResult<()>,
) -> AppResult<Vec<CheckOutcome>> {
    let mut outcomes = Vec::new();
    for (id, check) in doctor.selected(profiles)? {
        let outcome = CheckOutcome {
            id: id.to_string(),
            failure: evaluate(project_root, check, runner),
            fix: check.fix.clone(),
        };
        on_check(&outcome)?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// The failure reason for one check, or `None` when it passes.
fn evaluate(
    project_root: &AbsPath,
    check: &DoctorCheckConfig,
    runner: &dyn ToolRunner,
) -> Option<String> {
    if let Some(path) = &check.path {
        return (!project_root.as_path().join(path).exists()).then(|| format!("{path} is missing"));
    }
    let program = check.argv.first().map_or("", String::as_str);
    let invocation = ToolInvocation::new(check.argv.clone())
        .with_working_dir(project_root.as_path())
        .with_timeout(CHECK_TIMEOUT)
        .with_max_output_bytes(CHECK_OUTPUT_BYTES);
    let outcome = match runner.run(&invocation) {
        Ok(outcome) => outcome,
        Err(error) => return Some(format!("`{program}` could not run: {error}")),
    };
    if outcome.timed_out {
        return Some(format!(
            "`{program}` timed out after {}s",
            CHECK_TIMEOUT.as_secs()
        ));
    }
    if !outcome.succeeded() {
        let code = outcome
            .exit_code
            .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}"));
        // The command's last stderr line usually names the cause, such as which port is busy.
        let reason = outcome
            .stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| format!(": {}", line.trim()))
            .unwrap_or_default();
        return Some(format!(
            "`{}` failed ({code}){reason}",
            check.argv.join(" ")
        ));
    }
    let version = check.version.as_deref()?;
    let printed = format!("{}{}", outcome.stdout, outcome.stderr);
    if prints_version(&printed, version) {
        return None;
    }
    let seen = printed
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("nothing");
    Some(format!("expected {version}, found: {}", seen.trim()))
}

/// Whether `printed` contains `version` as whole numbers: `1.3.6` must not
/// match inside `1.3.60` or `11.3.6`, while a prefix such as `go1.26` or `v24.`
/// still matches `go1.26.1` or `v24.11.0`.
fn prints_version(printed: &str, version: &str) -> bool {
    let starts_with_digit = version.starts_with(|c: char| c.is_ascii_digit());
    let ends_with_digit = version.ends_with(|c: char| c.is_ascii_digit());
    printed.match_indices(version).any(|(start, _)| {
        let before = printed[..start].chars().next_back();
        let after = printed[start + version.len()..].chars().next();
        let left = !starts_with_digit || !before.is_some_and(|c| c.is_ascii_digit() || c == '.');
        let right = !ends_with_digit || !after.is_some_and(|c| c.is_ascii_digit());
        left && right
    })
}

#[cfg(test)]
mod tests {
    use toven_core::config::{DoctorCheckConfig, DoctorConfig};
    use toven_model::AbsPath;
    use toven_testkit::{FakeToolRunner, TestWorkspace};

    use super::run_checks;

    fn doctor(entries: Vec<(&str, DoctorCheckConfig)>) -> DoctorConfig {
        DoctorConfig {
            checks: entries
                .into_iter()
                .map(|(id, check)| (id.to_string(), check))
                .collect(),
        }
    }

    fn command(version: Option<&str>) -> DoctorCheckConfig {
        DoctorCheckConfig {
            argv: vec!["go".to_string(), "version".to_string()],
            version: version.map(ToString::to_string),
            fix: Some("mise install".to_string()),
            ..DoctorCheckConfig::default()
        }
    }

    fn failures(
        config: &DoctorConfig,
        runner: &FakeToolRunner,
        root: &AbsPath,
    ) -> Vec<Option<String>> {
        run_checks(root, config, &[], runner, &mut |_| Ok(()))
            .unwrap()
            .into_iter()
            .map(|outcome| outcome.failure)
            .collect()
    }

    #[test]
    fn a_command_check_passes_on_success_and_matching_version() {
        let workspace = TestWorkspace::new("doctor-checks-pass");
        let root = AbsPath::new(workspace.path()).unwrap();
        let runner = FakeToolRunner::new().with_stdout("go version go1.26.1 darwin/arm64\n");
        let config = doctor(vec![("go", command(Some("go1.26")))]);
        assert_eq!(failures(&config, &runner, &root), [None]);
    }

    #[test]
    fn a_version_matches_whole_numbers_only() {
        let workspace = TestWorkspace::new("doctor-checks-version-bounds");
        let root = AbsPath::new(workspace.path()).unwrap();
        let cases = [
            ("1.3.6", "1.3.6\n", true),
            ("1.3.6", "bun 1.3.6+abc\n", true),
            ("1.3.6", "1.3.60\n", false),
            ("1.3.6", "11.3.6\n", false),
            ("1.3.6", "1.3.60 then 1.3.6\n", true),
            ("go1.27.2", "go version go1.27.20\n", false),
            ("v24.", "v24.11.0\n", true),
        ];
        for (version, printed, passes) in cases {
            let runner = FakeToolRunner::new().with_stdout(printed);
            let config = doctor(vec![("tool", command(Some(version)))]);
            let failure = failures(&config, &runner, &root).remove(0);
            assert_eq!(
                failure.is_none(),
                passes,
                "{version} in {printed:?}: {failure:?}"
            );
        }
    }

    #[test]
    fn a_command_check_reports_wrong_version_exit_and_spawn_failures() {
        let workspace = TestWorkspace::new("doctor-checks-fail");
        let root = AbsPath::new(workspace.path()).unwrap();
        let config = doctor(vec![("go", command(Some("go1.27")))]);

        let wrong = FakeToolRunner::new().with_stdout("go version go1.26.1\n");
        let failure = failures(&config, &wrong, &root).remove(0).unwrap();
        assert!(
            failure.contains("expected go1.27, found: go version go1.26.1"),
            "{failure}"
        );

        let exit = FakeToolRunner::new().with_exit_code(Some(1));
        let failure = failures(&config, &exit, &root).remove(0).unwrap();
        assert!(failure.contains("failed (exit 1)"), "{failure}");

        let explained = FakeToolRunner::new()
            .with_exit_code(Some(1))
            .with_stderr("checking\nport 8443 is busy\n\n");
        let failure = failures(&config, &explained, &root).remove(0).unwrap();
        assert!(
            failure.ends_with("failed (exit 1): port 8443 is busy"),
            "{failure}"
        );

        let spawn = FakeToolRunner::new().with_spawn_failure("not found");
        let failure = failures(&config, &spawn, &root).remove(0).unwrap();
        assert!(failure.contains("could not run"), "{failure}");
    }

    #[test]
    fn a_path_check_requires_the_path() {
        let workspace = TestWorkspace::new("doctor-checks-path");
        workspace.write_file("target/REVISION", b"x").unwrap();
        let root = AbsPath::new(workspace.path()).unwrap();
        let check = |path: &str| DoctorCheckConfig {
            path: Some(path.to_string()),
            ..DoctorCheckConfig::default()
        };
        let config = doctor(vec![
            ("a", check("target/REVISION")),
            ("b", check("target/NOPE")),
        ]);
        let runner = FakeToolRunner::new();
        assert_eq!(
            failures(&config, &runner, &root),
            [None, Some("target/NOPE is missing".to_string())]
        );
    }
}
