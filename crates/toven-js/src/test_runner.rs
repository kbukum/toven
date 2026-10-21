//! The test runner a package's `test` script calls, and the flags that make it
//! write an LCOV tracefile.
//!
//! Onboarding renders a `coverage` task that runs each package's own `test`
//! script with these flags appended, so the runner writes `lcov.info` into the
//! package's Toven staging directory. A script is recognized only when it is a
//! single runner command; a chain (`a && b`) or a wrapper (`c8 node --test`)
//! would pass the flags to the wrong program, so it is not recognized.

use serde::{Deserialize, Serialize};

/// A test runner that can write LCOV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TestRunner {
    /// `vitest` (needs a coverage provider such as `@vitest/coverage-v8`).
    Vitest,
    /// `jest`.
    Jest,
    /// `bun test`.
    BunTest,
}

/// Shell syntax that makes a script more than one command.
const COMPOUND: [&str; 7] = ["&&", "||", ";", "|", ">", "`", "$("];

impl TestRunner {
    /// The runner `script` calls, when it is exactly one recognized runner
    /// command (optionally after `NAME=value` environment assignments).
    pub(crate) fn from_script(script: &str) -> Option<Self> {
        if COMPOUND.iter().any(|token| script.contains(token)) {
            return None;
        }
        let mut words = script
            .split_whitespace()
            .skip_while(|word| word.contains('=') && !word.starts_with('-'));
        match (words.next()?, words.next()) {
            ("vitest", None | Some("run")) => Some(Self::Vitest),
            ("vitest", Some(flag)) if flag.starts_with('-') => Some(Self::Vitest),
            ("jest", _) => Some(Self::Jest),
            ("bun", Some("test")) => Some(Self::BunTest),
            _ => None,
        }
    }

    /// The arguments that make the runner write `<dir>/lcov.info` and nothing
    /// else.
    pub(crate) fn lcov_args(self, dir: &str) -> Vec<String> {
        match self {
            Self::Vitest => vec![
                "--coverage.enabled".to_string(),
                "--coverage.reporter=lcovonly".to_string(),
                format!("--coverage.reportsDirectory={dir}"),
            ],
            Self::Jest => vec![
                "--coverage".to_string(),
                "--coverageReporters=lcovonly".to_string(),
                format!("--coverageDirectory={dir}"),
            ],
            Self::BunTest => vec![
                "--coverage".to_string(),
                "--coverage-reporter=lcov".to_string(),
                format!("--coverage-dir={dir}"),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TestRunner;

    #[test]
    fn recognizes_single_runner_commands() {
        let cases = [
            ("vitest", Some(TestRunner::Vitest)),
            ("vitest run", Some(TestRunner::Vitest)),
            ("vitest --passWithNoTests", Some(TestRunner::Vitest)),
            ("NODE_ENV=test vitest run", Some(TestRunner::Vitest)),
            ("jest", Some(TestRunner::Jest)),
            ("jest --runInBand src", Some(TestRunner::Jest)),
            ("bun test", Some(TestRunner::BunTest)),
            ("bun test ./src", Some(TestRunner::BunTest)),
        ];
        for (script, runner) in cases {
            assert_eq!(TestRunner::from_script(script), runner, "{script}");
        }
    }

    #[test]
    fn rejects_chains_wrappers_and_other_modes() {
        for script in [
            "tsc && vitest run",
            "vitest run; echo done",
            "c8 node --test",
            "node --test",
            "vitest bench",
            "vitest watch",
            "bun run test",
            "npm test --workspaces",
            "turbo run test",
            "",
        ] {
            assert_eq!(TestRunner::from_script(script), None, "{script}");
        }
    }

    #[test]
    fn lcov_args_name_the_report_directory() {
        assert_eq!(
            TestRunner::Jest.lcov_args("/out"),
            [
                "--coverage",
                "--coverageReporters=lcovonly",
                "--coverageDirectory=/out"
            ]
        );
        assert!(
            TestRunner::BunTest
                .lcov_args("/out")
                .contains(&"--coverage-dir=/out".to_string())
        );
        assert!(
            TestRunner::Vitest
                .lcov_args("/out")
                .contains(&"--coverage.reportsDirectory=/out".to_string())
        );
    }
}
