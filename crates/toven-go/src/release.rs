//! Go VCS-tag release target.
//!
//! Go modules have no manifest version and no registry publish step in Toven's
//! release model. The git tag is the version: the root module uses `vX.Y.Z`,
//! and submodules use `<repo-relative-module-root>/vX.Y.Z`. The Go module path
//! convention fixes the tag grammar, so a configured `tag_format` is rejected
//! as a misconfiguration rather than silently ignored.

use std::path::PathBuf;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_git::RefManager;
use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{Artifact, PublishOutcome, ReleaseMutation, ReleaseTarget, TagScheme};

/// Release target for Go modules released as git tags.
#[derive(Debug, Clone, Default)]
pub struct GoVcsTarget {
    /// Explicit repository working root; `None` resolves the process working
    /// directory (the engine runs from the repo root), mirroring
    /// `CratesIoTarget`.
    root: Option<PathBuf>,
}

impl GoVcsTarget {
    /// Construct the Go VCS-tag release target rooted at the process working
    /// directory.
    #[must_use]
    pub const fn new() -> Self {
        Self { root: None }
    }

    /// Pin the repository working root instead of resolving the process working
    /// directory — the explicit seam tests inject to avoid mutating
    /// process-global state.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    fn tags_for(&self) -> AppResult<Vec<String>> {
        let repo = rskit_git::discover(self.working_root()?)?;
        Ok(repo.list_tags()?.into_iter().map(|tag| tag.name).collect())
    }

    fn working_root(&self) -> AppResult<PathBuf> {
        self.root.clone().map_or_else(
            || {
                std::env::current_dir().map_err(|error| {
                    AppError::new(ErrorCode::Internal, "failed to read current directory")
                        .with_cause(error)
                })
            },
            Ok,
        )
    }
}

impl ReleaseTarget for GoVcsTarget {
    fn declared_version(&self, module: &Module) -> AppResult<Version> {
        Ok(self
            .published_versions(module)?
            .into_iter()
            .max()
            .unwrap_or_else(|| Version::new(0, 0, 0)))
    }

    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        let scheme = self.tag_scheme(module, None)?;
        let mut versions = self
            .tags_for()?
            .into_iter()
            .filter_map(|tag| scheme.parse(&tag))
            .collect::<Vec<_>>();
        versions.sort();
        versions.dedup();
        Ok(versions)
    }

    fn tag_scheme(&self, module: &Module, tag_format: Option<&str>) -> AppResult<TagScheme> {
        if tag_format.is_some() {
            return Err(AppError::invalid_input(
                "release.tag_format",
                "Go modules use the fixed Go module tag convention (root `vX.Y.Z`, submodule `<path>/vX.Y.Z`) and do not accept a tag_format override",
            ));
        }
        let root = module.root.as_path();
        let raw = root.display().to_string().replace('\\', "/");
        let normalized = raw.trim_matches('/');
        if normalized.is_empty() || normalized == "." {
            Ok(TagScheme::new("v", ""))
        } else {
            Ok(TagScheme::new(format!("{normalized}/v"), ""))
        }
    }

    fn package(&self, module: &Module) -> AppResult<Artifact> {
        Ok(Artifact::new(module.root.as_path()))
    }

    fn apply_release(&self, _module: &Module, _mutation: &ReleaseMutation) -> AppResult<()> {
        Ok(())
    }

    fn publish(&self, _module: &Module, _artifact: &Artifact) -> AppResult<PublishOutcome> {
        Ok(PublishOutcome::Published)
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::ReleaseTarget;
    use toven_testkit::git::GitScenario;

    use super::GoVcsTarget;

    fn module(name: &str, root: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("go").expect("ecosystem"), name).expect("module"),
            RepoPath::new(root).expect("root"),
        )
    }

    #[test]
    fn tag_scheme_uses_root_v_tags_and_submodule_path_tags() {
        let target = GoVcsTarget::new();

        assert_eq!(
            target
                .tag_scheme(&module("root", "."), None)
                .expect("root scheme")
                .format(&Version::new(1, 2, 3)),
            "v1.2.3"
        );
        assert_eq!(
            target
                .tag_scheme(&module("cache-redis", "cache/redis"), None)
                .expect("submodule scheme")
                .format(&Version::new(1, 2, 3)),
            "cache/redis/v1.2.3"
        );
    }

    #[test]
    fn configured_tag_format_is_rejected() {
        // The Go module tag convention fixes the grammar, so an explicit `tag_format`
        // is a misconfiguration — surface it as a typed error rather than silently
        // ignoring it.
        let error = GoVcsTarget::new()
            .tag_scheme(
                &module("cache-redis", "cache/redis"),
                Some("{module}/v{version}"),
            )
            .expect_err("configured tag_format rejected");

        assert!(error.to_string().contains("tag_format"));
    }

    #[test]
    fn published_versions_read_go_module_tags_from_git() {
        let workspace = toven_testkit::TestWorkspace::new("go-release-tags");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("go.mod", "module example.com/root\n", "initial")
            .expect("commit root");
        scenario.tag("v0.1.0", "root").expect("root tag");
        scenario
            .tag("cache/redis/v1.2.0", "redis")
            .expect("submodule tag");
        scenario
            .tag("cache/http/v9.9.9", "other")
            .expect("other tag");
        let target = GoVcsTarget::new().with_root(workspace.path());

        let versions = target
            .published_versions(&module("cache-redis", "cache/redis"))
            .expect("versions");

        assert_eq!(versions, vec![Version::new(1, 2, 0)]);
    }
}
