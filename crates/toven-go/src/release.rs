//! Go VCS-tag release target.
//!
//! Go modules have no manifest version and no registry publish step in Toven's
//! release model. The git tag is the version: the root module uses `vX.Y.Z`,
//! and submodules use `<repo-relative-module-root>/vX.Y.Z`. The Go module path
//! convention fixes the tag grammar, so a configured `tag_format` is rejected
//! as a misconfiguration rather than silently ignored.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::exists as file_exists;
use rskit_version::semver::Version;
use toven_model::{Module, RepoPath};
use toven_ports::{
    Artifact, BaselineSourceConfig, ManifestMutator, Packager, PublishOutcome, Publisher,
    ReleaseCredentials, ReleaseDefaults, ReleaseDefaultsSource, ReleaseMutation, SbomProducer,
    TagGrammar, TagMode, TagScheme, ToolInvocation, ToolRunner, VcsReader, VersionSource,
    Visibility,
};

use crate::exec::{go_command, run_go_json};

/// The `CycloneDX` Go SBOM tool Toven invokes argv-first for the Go `sbom`
/// phase (the [`SbomProducer`] implementation below).
const SBOM_TOOL: &str = "cyclonedx-gomod";

/// The `CycloneDX` JSON SBOM file suffix Toven writes (`<stem>.cdx.json`), the
/// same canonical name the Rust adapter and the engine's asset-staging use.
const SBOM_FILE_SUFFIX: &str = "cdx.json";

/// Hard bound on captured SBOM-tool output (256 KiB). The SBOM itself is written
/// to a file via `-output`, so only the tool's diagnostics flow through captured
/// output; this only guards against a pathological stream.
const MAX_SBOM_OUTPUT_BYTES: usize = 256 * 1024;

/// Timeout for one `cyclonedx-gomod` invocation. It resolves the module graph
/// (which may hit the module proxy), so it is wider than a local `go mod edit`.
const SBOM_TIMEOUT: Duration = Duration::from_mins(5);

/// The deterministic SBOM file stem for `module` — its short module name, with
/// any path/scheme separators folded to `-` so the produced artifact is a plain
/// file name the engine's declared-asset staging matches on.
fn sbom_stem(module: &Module) -> String {
    module.id.name.replace(['/', '\\', ':'], "-")
}

/// Build the argv-only `cyclonedx-gomod` invocation that writes the module's
/// `CycloneDX` JSON SBOM to `output`.
///
/// `mod` describes the module (not an application binary), `-json` selects the
/// `CycloneDX` JSON format, `-output` pins the destination so the tool writes
/// straight into Toven's bounded output directory (unlike `cargo cyclonedx`,
/// which writes next to the manifest), and the trailing directory scopes it to
/// exactly the module being released.
fn sbom_argv(output: &Path, module_dir: &Path) -> Vec<String> {
    vec![
        "mod".to_string(),
        "-json".to_string(),
        "-output".to_string(),
        output.display().to_string(),
        module_dir.display().to_string(),
    ]
}

/// Run one `cyclonedx-gomod` invocation, bounded and timed-out, surfacing a
/// timeout or non-zero exit as a typed error. The SBOM is written to a file, so
/// captured output carries only diagnostics.
fn run_sbom(invocation: ToolInvocation, runner: &dyn ToolRunner) -> AppResult<()> {
    let invocation = invocation
        .with_timeout(SBOM_TIMEOUT)
        .with_max_output_bytes(MAX_SBOM_OUTPUT_BYTES);
    let outcome = runner.run(&invocation)?;
    if outcome.timed_out {
        return Err(AppError::new(
            ErrorCode::Timeout,
            format!("`{SBOM_TOOL}` timed out"),
        ));
    }
    if !outcome.succeeded() {
        outcome.require_success("sbom tool `cyclonedx-gomod`")?;
    }
    Ok(())
}

/// Release target for Go modules released as git tags.
#[derive(Clone)]
pub struct GoVcsTarget {
    runner: Arc<dyn ToolRunner>,
    root: Option<PathBuf>,
    reachable_tags: Vec<String>,
}

impl GoVcsTarget {
    /// Construct the Go VCS-tag release target from the reachable tags gathered
    /// through the release orchestration's injected VCS reader.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>, reachable_tags: Vec<String>) -> Self {
        Self {
            runner,
            root: None,
            reachable_tags,
        }
    }

    /// Pin the repository working root instead of resolving the process working
    /// directory.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
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

/// The Go module tags reachable from `HEAD`, read through the VCS port.
///
/// A tag on an unmerged side branch is not a published version, so only tags
/// whose peeled commit is `HEAD` or a strict ancestor of `HEAD` are kept (the
/// equal-revision case is checked directly because [`VcsReader::is_ancestor`]
/// is strict).
pub(crate) fn reachable_tags(reader: &dyn VcsReader) -> AppResult<Vec<String>> {
    let head = reader.rev_parse("HEAD")?;
    let mut tags = Vec::new();
    for tag in reader.list_tags(None)? {
        let peeled = format!("refs/tags/{}^{{}}", tag.name);
        let tagged = reader.rev_parse(&peeled)?;
        if tagged == head || reader.is_ancestor(tagged.as_str(), "HEAD")? {
            tags.push(tag.name);
        }
    }
    Ok(tags)
}

impl VersionSource for GoVcsTarget {
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
            .reachable_tags
            .iter()
            .cloned()
            .filter_map(|tag| scheme.parse(&tag))
            .collect::<Vec<_>>();
        versions.sort();
        versions.dedup();
        Ok(versions)
    }

    fn version_in_manifest(&self, _manifest: &str) -> AppResult<Option<Version>> {
        // A Go module's version lives in its version-control tags, never in
        // `go.mod`, so there is no manifest-body version to anchor on. Go uses a
        // per-module tag baseline (not an umbrella-tag baseline), so this reader
        // is never consulted for Go; it returns `None` for contract completeness.
        Ok(None)
    }
}

impl TagGrammar for GoVcsTarget {
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
}

impl ReleaseDefaultsSource for GoVcsTarget {
    fn release_defaults(&self) -> ReleaseDefaults {
        // A Go module's per-module tag *is* its registry entry (`go get` reads
        // the tag), so each module anchors change detection on its own tag and
        // the train cuts only per-module tags — there is no umbrella registry to
        // consult and no aggregate repo tag to create.
        ReleaseDefaults::new(BaselineSourceConfig::OwnTag, TagMode::PerModule)
    }
}

impl Packager for GoVcsTarget {
    fn package(&self, module: &Module) -> AppResult<Artifact> {
        Ok(Artifact::new(module.root.as_path()))
    }
}

impl ManifestMutator for GoVcsTarget {
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
            let working_root = self.working_root()?;
            let invocation = go_command(
                [
                    "mod".to_string(),
                    "edit".to_string(),
                    format!("-require={import_path}@v{version}"),
                    manifest.display().to_string(),
                ],
                &working_root,
            );
            run_go_json(
                invocation,
                "go mod edit dependency floor",
                self.runner.as_ref(),
            )?;
        }
        let staged = RepoPath::new(manifest_rel).map_err(|error| {
            AppError::invalid_input("release.go_mod", error.to_string()).with_cause(error)
        })?;
        Ok(vec![staged])
    }
}

impl Publisher for GoVcsTarget {
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

impl SbomProducer for GoVcsTarget {
    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        let working_root = self.working_root()?;
        let module_dir = safe_join(&working_root, module.root.as_path()).map_err(|error| {
            AppError::invalid_input("release.sbom.module", error.to_string()).with_cause(error)
        })?;
        create_all(out_dir)?;
        // `cyclonedx-gomod` accepts `-output`, so it writes straight into the
        // bounded output directory under Toven's canonical `<stem>.cdx.json`
        // name — no next-to-manifest stray files to clean up afterwards.
        let artifact = out_dir.join(format!("{}.{SBOM_FILE_SUFFIX}", sbom_stem(module)));
        let argv = std::iter::once(SBOM_TOOL.to_string())
            .chain(sbom_argv(&artifact, &module_dir))
            .collect();
        let invocation = ToolInvocation::new(argv).with_working_dir(&working_root);
        run_sbom(invocation, self.runner.as_ref())?;
        if !file_exists(&artifact)? {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "`{SBOM_TOOL}` reported success but wrote no SBOM at '{}'",
                    artifact.display()
                ),
            ));
        }
        Ok(Some(Artifact::new(artifact)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{ManifestMutator, ReleaseMutation, TagGrammar, VersionSource};
    use toven_testkit::doubles::FakeToolRunner;
    use toven_testkit::git::GitScenario;
    use toven_vcs::RskitGitVcs;

    use super::GoVcsTarget;

    fn target() -> GoVcsTarget {
        GoVcsTarget::new(Arc::new(FakeToolRunner::new()), Vec::new())
    }

    fn target_with_reader(reader: &dyn toven_ports::VcsReader) -> GoVcsTarget {
        GoVcsTarget::new(
            Arc::new(FakeToolRunner::new()),
            super::reachable_tags(reader).expect("reachable tags"),
        )
    }

    fn module(name: &str, root: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("go").expect("ecosystem"), name).expect("module"),
            RepoPath::new(root).expect("root"),
        )
    }

    #[test]
    fn tag_scheme_uses_root_v_tags_and_submodule_path_tags() {
        let target = target();

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
    fn release_defaults_are_per_module_tag_anchored() {
        use toven_ports::{BaselineSourceConfig, ReleaseDefaultsSource, TagMode};

        // A Go module's per-module tag *is* its registry entry, so the baseline
        // anchors on each module's own tag and only per-module tags are cut.
        let defaults = target().release_defaults();
        assert_eq!(defaults.baseline, BaselineSourceConfig::OwnTag);
        assert_eq!(defaults.tag_mode, TagMode::PerModule);
    }

    #[test]
    fn configured_tag_format_is_rejected() {
        // The Go module tag convention fixes the grammar, so an explicit `tag_format`
        // is a misconfiguration — surface it as a typed error rather than silently
        // ignoring it.
        let error = target()
            .tag_scheme(
                &module("cache-redis", "cache/redis"),
                Some("{module}/v{version}"),
            )
            .expect_err("configured tag_format rejected");

        assert!(error.to_string().contains("tag_format"));
    }

    #[test]
    fn published_versions_read_go_module_tags_through_the_injected_reader() {
        use toven_ports::{Oid, TagRef};
        use toven_testkit::doubles::FakeVcsReader;

        // A scripted reader (no real git): every tag resolves to the same commit
        // as HEAD, so all are reachable, and the target reads its version tags
        // straight from the injected port — proving Go no longer bypasses the
        // seam to hit real git.
        let reader = FakeVcsReader::new()
            .with_rev_parse("c0ffee")
            .with_tags(vec![
                TagRef::new("cache/redis/v1.2.0", Oid::new("c0ffee")),
                TagRef::new("cache/redis/v1.3.0", Oid::new("c0ffee")),
                TagRef::new("cache/http/v9.9.9", Oid::new("c0ffee")),
            ]);
        let target = target_with_reader(&reader);

        let versions = target
            .published_versions(&module("cache-redis", "cache/redis"))
            .expect("versions");

        assert_eq!(versions, vec![Version::new(1, 2, 0), Version::new(1, 3, 0)]);
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
        let reader = RskitGitVcs::open(workspace.path()).expect("open repository");
        let target = target_with_reader(&reader).with_root(workspace.path());

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
        let reader = RskitGitVcs::open(workspace.path()).expect("open repository");
        let target = target_with_reader(&reader).with_root(workspace.path());

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
        let reader = RskitGitVcs::open(workspace.path()).expect("open repository");
        let target = target_with_reader(&reader);

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

        let error = target()
            .apply_release(&module("app", "app"), &mutation)
            .expect_err("missing Go import-path mapping rejected");

        assert!(error.to_string().contains("Go import paths"));
    }

    #[test]
    fn sbom_argv_is_an_argv_only_cyclonedx_gomod_invocation_scoped_to_the_module() {
        let argv = super::sbom_argv(
            Path::new("/out/api.cdx.json"),
            Path::new("/repo/services/api"),
        );
        assert_eq!(
            argv,
            vec![
                "mod".to_string(),
                "-json".to_string(),
                "-output".to_string(),
                "/out/api.cdx.json".to_string(),
                "/repo/services/api".to_string(),
            ]
        );
    }

    #[test]
    fn sbom_stem_is_the_short_module_name() {
        // Go module names are validated to be plain tokens (no separators), so
        // the stem is the module's short name and is a single file-name token
        // the engine's declared-asset staging matches on.
        assert_eq!(super::sbom_stem(&module("api", "services/api")), "api");
        assert_eq!(super::sbom_stem(&module("gokit", ".")), "gokit");
    }
}
