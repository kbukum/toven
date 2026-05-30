//! Git baseline resolution.

use std::path::PathBuf;

use rskit_git::Inspector;

use crate::core::{AppError, AppResult};

/// Resolved git baseline.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Baseline {
    /// Provider name.
    pub provider: String,
    /// Revision expression used for diffing.
    pub revision: String,
    /// Resolved object id.
    pub oid: String,
}

/// Context available to baseline providers.
#[derive(Debug, Clone)]
pub struct BaselineContext {
    /// Workspace root used to discover the repository.
    pub workspace_root: PathBuf,
}

/// Baseline provider contract.
pub trait BaselineProvider: Send + Sync {
    /// Provider name.
    fn name(&self) -> &'static str;
    /// Resolve a baseline.
    fn resolve(&self, ctx: &BaselineContext) -> AppResult<Baseline>;
}

/// Baseline provider that uses an explicit ref or SHA directly.
#[derive(Debug, Clone)]
pub struct ExplicitBaselineProvider {
    revision: String,
}

impl ExplicitBaselineProvider {
    /// Create an explicit baseline provider.
    #[must_use]
    pub fn new(revision: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
        }
    }
}

impl BaselineProvider for ExplicitBaselineProvider {
    fn name(&self) -> &'static str {
        "explicit"
    }

    fn resolve(&self, ctx: &BaselineContext) -> AppResult<Baseline> {
        let repo = rskit_git::discover(&ctx.workspace_root).map_err(|error| {
            AppError::invalid_input(
                "workspace.root",
                format!(
                    "failed to discover git repository from '{}'",
                    ctx.workspace_root.display()
                ),
            )
            .with_cause(error)
        })?;
        let oid = repo.rev_parse(&self.revision).map_err(|error| {
            AppError::invalid_input(
                "base",
                format!("failed to resolve git baseline '{}'", self.revision),
            )
            .with_cause(error)
        })?;
        Ok(Baseline {
            provider: self.name().to_string(),
            revision: self.revision.clone(),
            oid: oid.to_string(),
        })
    }
}

/// Baseline provider that resolves a configured git ref.
#[derive(Debug, Clone)]
pub struct GitRefBaselineProvider {
    revision: String,
}

impl GitRefBaselineProvider {
    /// Create a git-ref baseline provider.
    #[must_use]
    pub fn new(revision: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
        }
    }
}

impl BaselineProvider for GitRefBaselineProvider {
    fn name(&self) -> &'static str {
        "git-ref"
    }

    fn resolve(&self, ctx: &BaselineContext) -> AppResult<Baseline> {
        let mut baseline = ExplicitBaselineProvider::new(self.revision.clone()).resolve(ctx)?;
        baseline.provider = self.name().to_string();
        Ok(baseline)
    }
}

/// Baseline provider that resolves the merge-base between HEAD and a ref.
#[derive(Debug, Clone)]
pub struct MergeBaseBaselineProvider {
    reference: String,
}

impl MergeBaseBaselineProvider {
    /// Create a merge-base provider.
    #[must_use]
    pub fn new(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
        }
    }
}

impl BaselineProvider for MergeBaseBaselineProvider {
    fn name(&self) -> &'static str {
        "merge-base"
    }

    fn resolve(&self, ctx: &BaselineContext) -> AppResult<Baseline> {
        let repo = rskit_git::discover(&ctx.workspace_root).map_err(|error| {
            AppError::invalid_input(
                "workspace.root",
                format!(
                    "failed to discover git repository from '{}'",
                    ctx.workspace_root.display()
                ),
            )
            .with_cause(error)
        })?;
        repo.rev_parse(&self.reference).map_err(|error| {
            AppError::invalid_input(
                "base",
                format!("failed to resolve git ref '{}'", self.reference),
            )
            .with_cause(error)
        })?;
        let oid =
            rskit_git::LogReader::merge_base(&repo, "HEAD", &self.reference).map_err(|error| {
                AppError::invalid_input(
                    "base",
                    format!(
                        "failed to resolve merge-base of HEAD and '{}'",
                        self.reference
                    ),
                )
                .with_cause(error)
            })?;
        Ok(Baseline {
            provider: self.name().to_string(),
            revision: oid.to_string(),
            oid: oid.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use crate::git::baseline::{
        BaselineContext, BaselineProvider, ExplicitBaselineProvider, GitRefBaselineProvider,
        MergeBaseBaselineProvider,
    };

    #[test]
    fn explicit_provider_resolves_ref_to_oid() {
        let root = rskit_fs::TempDir::new().expect("temp dir");
        init_repo(root.path());
        let head = git_stdout(root.path(), ["rev-parse", "HEAD"]);
        let provider = ExplicitBaselineProvider::new("HEAD");

        let baseline = provider
            .resolve(&BaselineContext {
                workspace_root: root.path().to_path_buf(),
            })
            .expect("baseline resolves");

        assert_eq!(baseline.provider, "explicit");
        assert_eq!(baseline.revision, "HEAD");
        assert_eq!(baseline.oid, head);
    }

    #[test]
    fn git_ref_provider_marks_configured_ref_source() {
        let root = rskit_fs::TempDir::new().expect("temp dir");
        init_repo(root.path());
        let head = git_stdout(root.path(), ["rev-parse", "HEAD"]);
        let provider = GitRefBaselineProvider::new("HEAD");

        let baseline = provider
            .resolve(&BaselineContext {
                workspace_root: root.path().to_path_buf(),
            })
            .expect("baseline resolves");

        assert_eq!(baseline.provider, "git-ref");
        assert_eq!(baseline.oid, head);
    }

    #[test]
    fn merge_base_provider_resolves_common_ancestor() {
        let root = rskit_fs::TempDir::new().expect("temp dir");
        init_repo(root.path());
        let base = git_stdout(root.path(), ["rev-parse", "HEAD"]);
        git(root.path(), ["switch", "-c", "feature"]);
        fs::write(root.path().join("feature.txt"), "feature\n").expect("write feature");
        git(root.path(), ["add", "."]);
        git(root.path(), ["commit", "-m", "feature"]);
        let provider = MergeBaseBaselineProvider::new("main");

        let baseline = provider
            .resolve(&BaselineContext {
                workspace_root: root.path().to_path_buf(),
            })
            .expect("baseline resolves");

        assert_eq!(baseline.provider, "merge-base");
        assert_eq!(baseline.revision, base);
        assert_eq!(baseline.oid, base);
    }

    #[test]
    fn invalid_ref_reports_baseline_error() {
        let root = rskit_fs::TempDir::new().expect("temp dir");
        init_repo(root.path());
        let provider = ExplicitBaselineProvider::new("missing-ref");

        let error = provider
            .resolve(&BaselineContext {
                workspace_root: root.path().to_path_buf(),
            })
            .expect_err("missing ref fails");

        assert!(error.message.contains("invalid base"));
        assert!(error.message.contains("failed to resolve git baseline"));
    }

    fn init_repo(root: &Path) {
        git(root, ["init", "--initial-branch", "main"]);
        git(root, ["config", "user.email", "toven@example.invalid"]);
        git(root, ["config", "user.name", "Toven Test"]);
        fs::write(root.join("README.md"), "# fixture\n").expect("write readme");
        git(root, ["add", "."]);
        git(root, ["commit", "-m", "base"]);
    }

    fn git<const N: usize>(cwd: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N]) -> String {
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git stdout is utf-8")
            .trim()
            .to_string()
    }
}
