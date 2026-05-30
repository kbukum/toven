use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmokeCase {
    pub fixture: Option<String>,
    pub repo: Option<String>,
    pub copy_root: Option<String>,
    pub config: Option<toml::Value>,
    #[serde(default)]
    pub setup: Vec<SmokeSetup>,
    #[serde(default)]
    pub changes: Vec<SmokeChange>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub command: SmokeCommand,
    #[serde(default)]
    pub affected: bool,
    pub base: Option<String>,
    pub merge_base: Option<bool>,
    pub task: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub no_cache: bool,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub expect_status: Option<i32>,
    #[serde(default)]
    pub assert: SmokeAssertion,
    #[serde(default)]
    pub invocations: Vec<SmokeInvocation>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SmokeCommand {
    #[default]
    Plan,
    Affected,
    Run,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SmokeAssertion {
    #[default]
    Auto,
    CargoWaves,
    AffectedModules,
    None,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmokeInvocation {
    pub label: Option<String>,
    pub command: Option<SmokeCommand>,
    pub affected: Option<bool>,
    pub base: Option<String>,
    pub merge_base: Option<bool>,
    pub task: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    pub no_cache: Option<bool>,
    pub force: Option<bool>,
    pub expect_status: Option<i32>,
    #[serde(default)]
    pub assert: Option<SmokeAssertion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "kind")]
pub enum SmokeSetup {
    Write { path: String, content: String },
    Append { path: String, content: String },
    Delete { path: String },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmokeChange {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedInvocation {
    pub label: String,
    pub command: SmokeCommand,
    pub affected: bool,
    pub base: Option<String>,
    pub merge_base: Option<bool>,
    pub task: Option<String>,
    pub args: Vec<String>,
    pub no_cache: bool,
    pub force: bool,
    pub expect_status: i32,
    pub assert: SmokeAssertion,
}

impl SmokeCase {
    pub fn load(path: &Path) -> Self {
        let contents =
            fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let case: Self = toml::from_str(&contents)
            .unwrap_or_else(|err| panic!("parse {}: {err}", path.display()));
        assert!(
            case.fixture.is_some() || case.repo.is_some(),
            "{} must set fixture or repo",
            path.display()
        );
        case
    }

    pub fn invocations(&self) -> Vec<ResolvedInvocation> {
        if self.invocations.is_empty() {
            return vec![self.resolve_invocation(None, 0)];
        }

        self.invocations
            .iter()
            .enumerate()
            .map(|(index, invocation)| self.resolve_invocation(Some(invocation), index))
            .collect()
    }

    pub fn needs_git_fixture(&self) -> bool {
        self.affected
            || self.command == SmokeCommand::Run
            || self.base.is_some()
            || self.merge_base.is_some()
            || self.branch.is_some()
            || !self.changes.is_empty()
            || self.invocations.iter().any(|invocation| {
                invocation.command == Some(SmokeCommand::Run)
                    || invocation.affected.unwrap_or(false)
                    || invocation.base.is_some()
                    || invocation.merge_base.is_some()
            })
    }

    fn resolve_invocation(
        &self,
        invocation: Option<&SmokeInvocation>,
        index: usize,
    ) -> ResolvedInvocation {
        let command = invocation
            .and_then(|invocation| invocation.command)
            .unwrap_or(self.command);
        let affected = invocation
            .and_then(|invocation| invocation.affected)
            .unwrap_or(self.affected);
        let base = invocation
            .and_then(|invocation| invocation.base.clone())
            .or_else(|| self.base.clone());
        let merge_base = invocation
            .and_then(|invocation| invocation.merge_base)
            .or(self.merge_base);
        let task = invocation
            .and_then(|invocation| invocation.task.clone())
            .or_else(|| self.task.clone());
        let args = invocation
            .and_then(|invocation| invocation.args.clone())
            .unwrap_or_else(|| self.args.clone());
        let no_cache = invocation
            .and_then(|invocation| invocation.no_cache)
            .unwrap_or(self.no_cache);
        let force = invocation
            .and_then(|invocation| invocation.force)
            .unwrap_or(self.force);
        let expect_status = invocation
            .and_then(|invocation| invocation.expect_status)
            .or(self.expect_status)
            .unwrap_or(0);
        let assert = invocation
            .and_then(|invocation| invocation.assert)
            .unwrap_or(self.assert);
        let label = invocation
            .and_then(|invocation| invocation.label.clone())
            .unwrap_or_else(|| default_label(command, index));

        if command == SmokeCommand::Run {
            assert!(task.is_some(), "run invocation {label} must set task");
        }

        ResolvedInvocation {
            label,
            command,
            affected,
            base,
            merge_base,
            task,
            args,
            no_cache,
            force,
            expect_status,
            assert,
        }
    }
}

impl ResolvedInvocation {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn should_assert_cargo_waves(&self) -> bool {
        match self.assert {
            SmokeAssertion::CargoWaves => true,
            SmokeAssertion::AffectedModules | SmokeAssertion::None => false,
            SmokeAssertion::Auto => self.command == SmokeCommand::Plan && !self.affected,
        }
    }

    pub fn should_assert_affected_modules(&self) -> bool {
        match self.assert {
            SmokeAssertion::AffectedModules => true,
            SmokeAssertion::CargoWaves | SmokeAssertion::None => false,
            SmokeAssertion::Auto => self.command == SmokeCommand::Affected || self.affected,
        }
    }
}

pub fn managed_case_paths(cases_dir: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(cases_dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", cases_dir.display()))
        .map(|entry| entry.expect("read case entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn default_label(command: SmokeCommand, index: usize) -> String {
    let command = match command {
        SmokeCommand::Plan => "plan",
        SmokeCommand::Affected => "affected",
        SmokeCommand::Run => "run",
    };
    if index == 0 {
        command.to_owned()
    } else {
        format!("{command}-{index}")
    }
}
