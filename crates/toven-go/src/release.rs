//! Go VCS-tag release target.
//!
//! Go modules have no manifest version and no registry publish step in Toven's
//! release model. The git tag is the version: the root module uses `vX.Y.Z`,
//! and submodules use `<repo-relative-module-root>/vX.Y.Z`. The Go module path
//! convention fixes the tag grammar, so a configured `tag_format` is rejected
//! as a misconfiguration rather than silently ignored.

use std::path::PathBuf;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_git::{Inspector, LogReader, RefManager};
use rskit_process::ProcessSpec;
use rskit_version::semver::Version;
use toven_model::{Module, RepoPath};
use toven_ports::{
    Artifact, PublishOutcome, ReleaseCredentials, ReleaseMutation, ReleaseTarget, TagScheme,
    Visibility,
};

use crate::exec::run_go_json;

/// Release target for Go modules released as git tags.
#[derive(Debug, Clone, Default)]
pub struct GoVcsTarget {
    /// Explicit repository working root; `None` resolves the process working
    /// directory (the engine runs from the repo root), mirroring
    /// `CargoRegistryTarget`.
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

    fn reachable_tags_for(&self) -> AppResult<Vec<String>> {
        let repo = rskit_git::discover(self.working_root()?)?;
        let head = repo.rev_parse("HEAD")?;
        let mut tags = Vec::new();
        for tag in repo.list_tags()? {
            let peeled = format!("refs/tags/{}^{{}}", tag.name);
            let tagged = repo.rev_parse(&peeled)?;
            if tagged == head || repo.is_ancestor(&tagged.to_string(), "HEAD")? {
                tags.push(tag.name);
            }
        }
        Ok(tags)
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
        self.published_versions(module)?.into_iter().max().ok_or_else(|| {
            AppError::invalid_input(
                "release.tags",
                format!(
                    "Go module '{}' has no reachable release tag; set an explicit release version before the first Go release",
                    module.key()
                ),
            )
        })
    }

    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        let scheme = self.tag_scheme(module, None)?;
        let mut versions = self
            .reachable_tags_for()?
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

    fn apply_release(
        &self,
        module: &Module,
        mutation: &ReleaseMutation,
    ) -> AppResult<Vec<RepoPath>> {
        if mutation.dep_floor_updates.len() != mutation.dep_floor_import_updates.len() {
            return Err(AppError::invalid_input(
                "release.go_mod",
                format!(
                    "Go dependency rewrites require Go import paths, but module '{}' has {} \
                     dependency floor update(s) without a matching import path",
                    module.key(),
                    mutation.dep_floor_updates.len() - mutation.dep_floor_import_updates.len()
                ),
            ));
        }
        // Go carries no version in a manifest: a plain version cut writes nothing
        // and returns no staged paths, so the engine tags `HEAD` rather than
        // fabricating an empty commit. Only a dependency-floor rewrite touches
        // `go.mod`, which is then the one path the release commit stages.
        if mutation.dep_floor_import_updates.is_empty() {
            return Ok(Vec::new());
        }
        let manifest_rel = module.manifest.as_ref().map_or_else(
            || module.root.as_path().join("go.mod"),
            |path| path.as_path().to_path_buf(),
        );
        let manifest = safe_join(&self.working_root()?, &manifest_rel).map_err(|error| {
            AppError::invalid_input("release.go_mod", error.to_string()).with_cause(error)
        })?;
        for (import_path, version) in &mutation.dep_floor_import_updates {
            let spec = ProcessSpec::new("go")
                .arg("mod")
                .arg("edit")
                .arg(format!("-require={import_path}@v{version}"))
                .arg(&manifest)
                .dir(self.working_root()?);
            run_go_json(&spec, "go mod edit dependency floor")?;
        }
        let staged = RepoPath::new(manifest_rel).map_err(|error| {
            AppError::invalid_input("release.go_mod", error.to_string()).with_cause(error)
        })?;
        Ok(vec![staged])
    }

    fn publish(
        &self,
        _module: &Module,
        _artifact: &Artifact,
        _credentials: &ReleaseCredentials,
        _visibility: Visibility,
    ) -> AppResult<PublishOutcome> {
        // Go modules publish by tag, not to a package registry, so a Go release's
        // exposure follows the repository the tag is pushed to; there is no
        // separate registry visibility for this target to honor or reject.
        Ok(PublishOutcome::Published)
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{ReleaseMutation, ReleaseTarget};
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

    #[test]
    fn published_versions_ignore_unreachable_tags() {
        let workspace = toven_testkit::TestWorkspace::new("go-release-reachable-tags");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        let main = scenario
            .commit_file("go.mod", "module example.com/root\n", "initial")
            .expect("commit root")
            .to_string();
        scenario.tag("v1.0.0", "root").expect("root tag");
        scenario.branch_and_checkout("side").expect("side branch");
        scenario
            .commit_file("side.txt", "side\n", "side")
            .expect("side commit");
        scenario.tag("v9.9.9", "side").expect("side tag");
        scenario.checkout(&main).expect("return to main commit");
        let target = GoVcsTarget::new().with_root(workspace.path());

        let versions = target
            .published_versions(&module("root", "."))
            .expect("versions");

        assert_eq!(versions, vec![Version::new(1, 0, 0)]);
    }

    #[test]
    fn declared_version_rejects_a_module_without_a_reachable_release_tag() {
        let workspace = toven_testkit::TestWorkspace::new("go-release-no-tags");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("go.mod", "module example.com/root\n", "initial")
            .expect("commit root");
        let target = GoVcsTarget::new().with_root(workspace.path());

        let error = target
            .declared_version(&module("root", "."))
            .expect_err("missing tag rejected");

        assert!(error.to_string().contains("reachable release tag"));
    }

    #[test]
    fn apply_release_rejects_dependency_rewrites_without_import_paths() {
        let mut mutation = ReleaseMutation::version(Version::new(1, 1, 0));
        mutation
            .dep_floor_updates
            .insert(module("core", "core").id, Version::new(1, 1, 0));

        let error = GoVcsTarget::new()
            .apply_release(&module("app", "app"), &mutation)
            .expect_err("missing Go import-path mapping rejected");

        assert!(error.to_string().contains("Go import paths"));
    }
}
