//! Project health checks: the ecosystems' own checks, then the
//! `[doctor.checks.<id>]` predicates.
//!
//! An ecosystem check arrives decided (an offline file check) or as a command.
//! A configured check is a path that must exist or an argv command. A command
//! must succeed (and, with `version`, print that text). Commands run through the injected
//! [`ToolRunner`] with a bounded timeout and output cap, never a shell. This
//! module classifies outcomes; the CLI reports them.

use std::path::Path;
use std::time::Duration;

use rskit_errors::AppResult;
use toven_core::config::{DoctorCheckConfig, DoctorConfig};
use toven_model::AbsPath;
use toven_ports::{CheckVerdict, EcosystemCheck, ToolInvocation, ToolRunner};

/// How long one check command may run.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Output kept from one check command (enough for any version banner).
const CHECK_OUTPUT_BYTES: usize = 64 * 1024;

/// One evaluated health check.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckOutcome {
    /// The check id: `<ecosystem>/<id>` for an ecosystem check, or the
    /// `[doctor.checks.<id>]` key.
    pub id: String,
    /// Why the check failed; `None` when it passed.
    pub failure: Option<String>,
    /// The fix hint.
    pub fix: Option<String>,
}

impl CheckOutcome {
    /// Whether the check passed.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.failure.is_none()
    }
}

/// Run every ecosystem check, then the configured checks `profiles` selects,
/// reporting each as it completes. Ecosystem checks belong to no profile and
/// always run.
///
/// # Errors
/// Returns a typed error for an unknown profile, or propagates a failure from
/// `on_check`. A failing check is an outcome, not an error.
pub fn run_checks(
    project_root: &AbsPath,
    doctor: &DoctorConfig,
    profiles: &[String],
    ecosystem: &[EcosystemCheck],
    runner: &dyn ToolRunner,
    on_check: &mut dyn FnMut(&CheckOutcome) -> AppResult<()>,
) -> AppResult<Vec<CheckOutcome>> {
    let selected = doctor.selected(profiles)?;
    let mut outcomes = Vec::new();
    for check in ecosystem {
        let failure = match &check.verdict {
            CheckVerdict::Passed => None,
            CheckVerdict::Failed(reason) => Some(reason.clone()),
            CheckVerdict::Command { argv, dir, version } => run_command(
                &project_root.as_path().join(dir.as_path()),
                argv,
                version.as_deref(),
                runner,
            ),
        };
        let outcome = CheckOutcome {
            id: check.id.clone(),
            failure,
            fix: check.fix.clone(),
        };
        on_check(&outcome)?;
        outcomes.push(outcome);
    }
    for (id, check) in selected {
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
    run_command(
        project_root.as_path(),
        &check.argv,
        check.version.as_deref(),
        runner,
    )
}

/// The failure reason for one command check run in `dir`, or `None` when it
/// succeeds and prints `version`.
fn run_command(
    dir: &Path,
    argv: &[String],
    version: Option<&str>,
    runner: &dyn ToolRunner,
) -> Option<String> {
    let program = argv.first().map_or("", String::as_str);
    let invocation = ToolInvocation::new(argv.to_vec())
        .with_working_dir(dir)
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
        return Some(format!("`{}` failed ({code}){reason}", argv.join(" ")));
    }
    let version = version?;
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
    use toven_model::{AbsPath, RepoPath};
    use toven_ports::{CheckVerdict, EcosystemCheck};
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
        run_checks(root, config, &[], &[], runner, &mut |_| Ok(()))
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

    #[test]
    fn ecosystem_checks_run_first_and_command_checks_run_in_their_directory() {
        let workspace = TestWorkspace::new("doctor-checks-ecosystem");
        let root = AbsPath::new(workspace.path()).unwrap();
        let ecosystem = [
            EcosystemCheck::new("js/install", CheckVerdict::Failed("stale".to_string()))
                .with_fix("npm ci"),
            EcosystemCheck::new("js/ok", CheckVerdict::Passed),
            EcosystemCheck::new(
                "js/web/package-manager",
                CheckVerdict::Command {
                    argv: vec!["pnpm".to_string(), "--version".to_string()],
                    dir: RepoPath::new("web").unwrap(),
                    version: Some("10.2.0".to_string()),
                },
            ),
        ];
        let config = doctor(vec![("go", command(None))]);
        let runner = FakeToolRunner::new().with_stdout("9.15.0\n");

        let outcomes =
            run_checks(&root, &config, &[], &ecosystem, &runner, &mut |_| Ok(())).unwrap();

        let ids: Vec<&str> = outcomes.iter().map(|outcome| outcome.id.as_str()).collect();
        assert_eq!(ids, ["js/install", "js/ok", "js/web/package-manager", "go"]);
        assert_eq!(outcomes[0].failure.as_deref(), Some("stale"));
        assert_eq!(outcomes[0].fix.as_deref(), Some("npm ci"));
        assert!(outcomes[1].passed());
        assert_eq!(
            outcomes[2].failure.as_deref(),
            Some("expected 10.2.0, found: 9.15.0")
        );
        assert_eq!(
            runner.requests()[0].working_dir(),
            Some(workspace.path().join("web").as_path())
        );
    }
}
