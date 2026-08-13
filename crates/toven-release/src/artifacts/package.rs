//! `release package` verb: turn already-built binaries into the fixed-name
//! archive assets the hosted-release config declares.
//!
//! The engine owns release *policy* — which declared asset a built binary maps
//! to, which archive format that asset name implies, and that the produced
//! bytes are deterministic — while [`rskit_fs::archive`] owns the mechanical
//! packaging. The verb is non-mutating: it never bumps a manifest, tags, or
//! publishes. It consumes one already-built target's binary (cross-target
//! compilation stays a CI matrix concern) and stages exactly the archive asset
//! `[…release.host].assets` declares for that target.
//!
//! Asset → binary mapping is derived, not configured twice: for a target triple
//! `T`, the archive extension is `zip` for a `*windows*` triple and `tar.gz`
//! otherwise, and the one declared asset whose file name ends `-{T}.{ext}` is
//! the output. Its base name (the file name minus that suffix) is the member
//! recorded in the archive — with a `.exe` suffix on Windows — and the binary
//! archived defaults to `target/{T}/release/{member}`. Every failure is fail
//! closed with a typed error: an unknown/traversing target triple, no declared
//! asset for the target, or a missing built binary.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::archive::{self, ArchiveEntry};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::{exists as file_exists, remove_if_exists};
use toven_model::ReleasePhase;
use toven_ports::{Provider, Reporter, ToolRunner};

use crate::hosting::run_delegated_preview;
use crate::planning::plan::{release_targets, resolve_release_settings};
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

/// Unix mode recorded for a packaged executable (`rwxr-xr-x`).
const EXECUTABLE_MODE: u32 = 0o755;

/// The archive format a declared asset name selects.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ArchiveFormat {
    /// A gzip-compressed tar archive (`.tar.gz`), used for non-Windows targets.
    TarGz,
    /// A zip archive (`.zip`), used for Windows targets.
    Zip,
}

impl ArchiveFormat {
    /// The format implied by a target triple: `Zip` for a `*windows*` triple,
    /// `TarGz` otherwise.
    #[must_use]
    fn for_target(target: &str) -> Self {
        if target.contains("windows") {
            Self::Zip
        } else {
            Self::TarGz
        }
    }

    /// The file-name extension (without a leading dot) for the format.
    #[must_use]
    const fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }

    /// The canonical format name for typed reporting.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

/// One archive asset the verb produced from a built binary.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackagedAsset {
    /// The project-relative asset path written (as declared in `host.assets`).
    pub asset: String,
    /// The built binary archived into the asset. For a delegated backing this
    /// is the produced archive path itself, since the external tool owns the
    /// build-and-archive step.
    pub source: PathBuf,
    /// The archive format written.
    pub format: ArchiveFormat,
    /// The produced archive size in bytes.
    pub bytes: u64,
    /// How the asset was produced: `native` (Toven archived a built binary) or
    /// `delegated` (an external tool produced it and Toven normalized it back).
    pub backing: &'static str,
}

/// The typed result of `release package` for one target.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackageReport {
    /// The target triple that was packaged.
    pub target: String,
    /// The archive assets produced (usually exactly one).
    pub assets: Vec<PackagedAsset>,
}

impl PackageReport {
    /// Construct a package report.
    #[must_use]
    pub const fn new(target: String, assets: Vec<PackagedAsset>) -> Self {
        Self { target, assets }
    }
}

/// Stage the declared archive assets for `target`.
///
/// Each declared asset is produced by its owning module's `Package` backing: a
/// **native** module has Toven archive an already-built binary (`binary_override`
/// supplies it explicitly; otherwise it is located at
/// `target/<target>/release/<member>`), while a **delegated** module has an
/// external tool (e.g. `GoReleaser`) produce the archive — Toven runs the tool's
/// mutation-free preview once through `delegated`, then normalizes the produced
/// archive back into the typed report. Toven owns which asset maps to which
/// target and that the result exists in both backings.
///
/// # Errors
/// Fails closed with a typed error when the target triple is malformed, no
/// hosted-release asset is declared for the target, the native built binary is
/// missing, or a delegated tool fails or fails to produce a declared archive —
/// as well as propagating configuration, discovery, graph, archive, and I/O
/// failures.
pub fn release_package(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    target: &str,
    binary_override: Option<&Path>,
    tool_runner: &dyn ToolRunner,
    reporter: &mut dyn Reporter,
) -> AppResult<PackageReport> {
    validate_target(target)?;
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let settings = resolve_release_settings(&context, &targets)?;

    if crate::artifacts::assets::declared_release_assets(&settings).is_empty() {
        return Err(AppError::invalid_input(
            "release.host.assets",
            "no hosted-release assets are declared; nothing to package (set \
             […release.host] forge + assets)",
        ));
    }

    let format = ArchiveFormat::for_target(target);
    let suffix = format!("-{target}.{}", format.extension());
    // Resolve each declared archive asset matching the target to its owning
    // module's Package backing, first-owner-wins in `ModuleKey` order so a
    // shared asset is produced once and deterministically.
    let matched = resolve_matched_assets(&settings, &suffix)?;
    if matched.is_empty() {
        return Err(AppError::invalid_input(
            "release.host.assets",
            format!(
                "no hosted-release asset is declared for target '{target}' (expected an asset \
                 named '*{suffix}')"
            ),
        ));
    }

    let project_root = request.project_root.as_path();
    // Clear each delegated owner's declared archive before its tool runs, so the
    // post-preview existence check proves *this* run produced it. Without this, a
    // stale archive from a prior run plus a tool that exits 0 without rewriting
    // would be silently accepted — undermining the fail-closed guarantee.
    for owner in &matched {
        if matches!(owner.backing, AssetBacking::Delegated(_)) {
            remove_if_exists(&safe_join_asset(project_root, owner.asset)?)?;
        }
    }
    // Run each distinct delegated tool's preview once — a single preview (e.g.
    // GoReleaser's `--snapshot`) produces every archive that tool owns, so
    // re-running it per matched asset would repeat a multi-minute build. Dedup
    // on the fully-resolved argv so two identically-configured tools collapse to
    // one run while genuinely distinct invocations each run once.
    let mut previewed: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
    for owner in &matched {
        if let AssetBacking::Delegated(tool) = &owner.backing
            && previewed.insert(delegated_preview_key(tool))
        {
            run_delegated_preview(ReleasePhase::Package, tool, tool_runner, project_root)?;
        }
    }

    let mut assets = Vec::with_capacity(matched.len());
    for owner in &matched {
        assets.push(match &owner.backing {
            AssetBacking::Native => package_asset_native(
                project_root,
                owner.asset,
                target,
                format,
                &suffix,
                binary_override,
            )?,
            AssetBacking::Delegated(_) => {
                package_asset_delegated(project_root, owner.asset, format)?
            }
        });
    }
    assets.sort_by(|left, right| left.asset.cmp(&right.asset));
    Ok(PackageReport::new(target.to_string(), assets))
}

/// How a declared asset is backed: archived natively by Toven, or produced by a
/// delegated external tool.
enum AssetBacking {
    /// Toven archives an already-built binary.
    Native,
    /// An external tool produces the archive.
    Delegated(toven_ports::DelegatedTool),
}

/// One declared asset resolved to its owning module's Package backing.
struct MatchedAsset<'a> {
    /// The project-relative declared asset path.
    asset: &'a str,
    /// The owning module's Package backing.
    backing: AssetBacking,
}

/// A stable identity for a delegated tool's preview invocation: its
/// fully-resolved preview argv (tool name + preview arguments).
///
/// Two delegated owners that resolve the same tool and preview arguments share
/// one preview run — the tool produces every archive once — while genuinely
/// distinct invocations each run. Keying on the argv, not just the tool name,
/// keeps differently-configured previews of the same executable distinct.
fn delegated_preview_key(tool: &toven_ports::DelegatedTool) -> Vec<String> {
    let mut key = Vec::with_capacity(tool.preview.len() + 1);
    key.push(tool.tool.clone());
    key.extend(tool.preview.iter().cloned());
    key
}

/// Resolve every declared archive asset matching `suffix` to its owning module's
/// Package backing, first-owner-wins in `ModuleKey` order so a shared asset is
/// produced once and deterministically.
fn resolve_matched_assets<'a>(
    settings: &'a std::collections::BTreeMap<
        toven_model::ModuleKey,
        crate::ResolvedReleaseSettings,
    >,
    suffix: &str,
) -> AppResult<Vec<MatchedAsset<'a>>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut matched = Vec::new();
    for resolved in settings.values() {
        let backing = resolved.phase_backing(ReleasePhase::Package)?;
        let tool = resolved.delegated_tool(ReleasePhase::Package).cloned();
        for asset in &resolved.host.assets {
            if !asset_file_name(asset).is_some_and(|name| name.ends_with(suffix)) {
                continue;
            }
            if !seen.insert(asset.as_str()) {
                continue;
            }
            let backing = if backing.is_native() {
                AssetBacking::Native
            } else {
                // A delegated Package backing always carries its tool: the
                // config loader rejects a delegated backing without one, and
                // the plan guard rejects a delegated non-delegable phase.
                AssetBacking::Delegated(tool.clone().ok_or_else(|| {
                    AppError::new(
                        ErrorCode::Internal,
                        format!(
                            "asset '{asset}' delegates the package phase but resolved no \
                             delegated tool"
                        ),
                    )
                })?)
            };
            matched.push(MatchedAsset { asset, backing });
        }
    }
    Ok(matched)
}

/// Normalize a delegated tool's produced archive back into the typed report:
/// verify the declared asset exists on disk and record its size, failing closed
/// when the tool did not produce it.
fn package_asset_delegated(
    project_root: &Path,
    asset: &str,
    format: ArchiveFormat,
) -> AppResult<PackagedAsset> {
    let out = safe_join_asset(project_root, asset)?;
    if !file_exists(&out)? {
        return Err(AppError::invalid_input(
            "release.package.delegated",
            format!(
                "delegated package tool did not produce the declared asset '{asset}' at '{}'",
                out.display()
            ),
        ));
    }
    let bytes = std::fs::metadata(&out)
        .map(|meta| meta.len())
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("cannot stat produced archive '{}': {error}", out.display()),
            )
            .with_cause(error)
        })?;
    Ok(PackagedAsset {
        asset: asset.to_string(),
        source: out,
        format,
        bytes,
        backing: "delegated",
    })
}

/// Package a single declared asset from its built binary.
fn package_asset_native(
    project_root: &Path,
    asset: &str,
    target: &str,
    format: ArchiveFormat,
    suffix: &str,
    binary_override: Option<&Path>,
) -> AppResult<PackagedAsset> {
    let file_name = asset_file_name(asset).ok_or_else(|| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' has no file name to package"),
        )
    })?;
    // The base name (file name minus the `-{target}.{ext}` suffix) is the
    // binary/member name; Windows binaries carry the `.exe` extension.
    let base = file_name.strip_suffix(suffix).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!("asset '{asset}' no longer matches the '{suffix}' suffix"),
        )
    })?;
    let member = match format {
        ArchiveFormat::Zip => format!("{base}.exe"),
        ArchiveFormat::TarGz => base.to_string(),
    };

    let source = binary_override.map_or_else(
        || {
            project_root
                .join("target")
                .join(target)
                .join("release")
                .join(&member)
        },
        Path::to_path_buf,
    );
    if !file_exists(&source)? {
        return Err(AppError::invalid_input(
            "release.package.binary",
            format!(
                "built binary not found at '{}' for target '{target}'; build it before packaging",
                source.display()
            ),
        ));
    }

    let out = safe_join_asset(project_root, asset)?;
    if let Some(parent) = out.parent() {
        create_all(parent)?;
    }

    let entries = [ArchiveEntry::new(member, source.clone(), EXECUTABLE_MODE)];
    match format {
        ArchiveFormat::TarGz => archive::tar_gz(&entries, &out)?,
        ArchiveFormat::Zip => archive::zip(&entries, &out)?,
    }

    let bytes = std::fs::metadata(&out)
        .map(|meta| meta.len())
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("cannot stat produced archive '{}': {error}", out.display()),
            )
            .with_cause(error)
        })?;
    Ok(PackagedAsset {
        asset: asset.to_string(),
        source,
        format,
        bytes,
        backing: "native",
    })
}

/// The final path component of a project-relative asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}

/// Resolve a declared asset to its safe absolute path under `project_root`,
/// rejecting a path that would traverse outside the project.
fn safe_join_asset(project_root: &Path, asset: &str) -> AppResult<PathBuf> {
    safe_join(project_root, asset).map_err(|error| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })
}

/// Reject a target triple that is empty or carries any character outside the
/// triple alphabet, closing the path-traversal hole a crafted `target` would
/// otherwise open when it is joined into `target/<triple>/release`.
fn validate_target(target: &str) -> AppResult<()> {
    if target.is_empty()
        || !target.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(AppError::invalid_input(
            "release.package.target",
            format!(
                "invalid target triple '{target}' (expected e.g. \
                 x86_64-unknown-linux-gnu: lowercase letters, digits, '_', '-')"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_config::RawValue;
    use rskit_fs::TempDir;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeToolRunner, RecordingReporter,
    };

    use super::{ArchiveFormat, release_package};
    use toven_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_core::plan::PlanRequest;

    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const WINDOWS: &str = "x86_64-pc-windows-msvc";

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(eid("rust"), name).unwrap(),
            RepoPath::new(format!("crates/{name}")).unwrap(),
        )
    }

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid("rust"), RawValue::from(json!({ "release": {} })));
        Document {
            project: ProjectConfig {
                name: "demo".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems,
            modules: BTreeMap::new(),
            members: Vec::new(),
            hooks: std::collections::BTreeMap::new(),
            units: std::collections::BTreeMap::new(),
        }
    }

    fn request(root: &Path) -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new(root.to_str().unwrap()).unwrap(),
        )
    }

    /// Build a provider whose ecosystem declares `assets` as hosted-release
    /// assets on a `github` forge.
    fn provider_with_assets(assets: Vec<&str>) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".to_string()),
                    assets: Some(assets.into_iter().map(str::to_string).collect()),
                    ..HostConfig::default()
                }),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new())
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    /// Build a provider whose ecosystem declares `assets` and backs the
    /// `package` phase with a delegated `goreleaser` tool.
    fn provider_with_delegated_package(assets: Vec<&str>) -> FakeProvider {
        use std::collections::BTreeMap;

        use toven_model::ReleasePhase;
        use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let mut phases = BTreeMap::new();
        phases.insert(
            ReleasePhase::Package,
            PhaseConfig {
                backing: PhaseBackingKind::Delegated,
                delegated: Some(DelegatedTool {
                    tool: "goreleaser".into(),
                    args: Some(vec!["release".into(), "--clean".into()]),
                    preview: vec!["release".into(), "--snapshot".into(), "--clean".into()],
                }),
            },
        );
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".to_string()),
                    assets: Some(assets.into_iter().map(str::to_string).collect()),
                    ..HostConfig::default()
                }),
                phases: Some(PhasesConfig(phases)),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new())
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }
    /// Write a fake built binary at `target/<triple>/release/<name>` under
    /// `root`, returning its path.
    fn write_built_binary(root: &Path, triple: &str, name: &str) {
        let dir = root.join("target").join(triple).join("release");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), b"fake-toven-binary").unwrap();
    }

    #[test]
    fn packages_the_declared_tar_gz_from_the_built_binary() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), LINUX, "toven");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.target, LINUX);
        assert_eq!(report.assets.len(), 1);
        let asset = &report.assets[0];
        assert_eq!(asset.asset, "dist/toven-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(asset.format, ArchiveFormat::TarGz);
        assert!(asset.bytes > 0);
        assert_eq!(asset.backing, "native");
        let produced = root
            .path()
            .join("dist/toven-x86_64-unknown-linux-gnu.tar.gz");
        assert!(
            produced.is_file(),
            "archive must be written to the asset path"
        );
    }

    #[test]
    fn delegated_package_runs_the_tool_preview_and_normalizes_the_produced_archive() {
        let root = TempDir::new().unwrap();
        let asset_rel = "dist/toven-x86_64-unknown-linux-gnu.tar.gz";
        // No native binary is built: the delegated tool "produces" the archive
        // instead, which the runner writes on a successful preview run.
        let runner = FakeToolRunner::new()
            .with_produced_file(root.path().join(asset_rel), b"goreleaser-archive-bytes");
        let provider = provider_with_delegated_package(vec![asset_rel]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &runner,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.assets.len(), 1);
        let asset = &report.assets[0];
        assert_eq!(asset.asset, asset_rel);
        assert_eq!(asset.backing, "delegated");
        assert!(asset.bytes > 0);
        // The tool was driven argv-first as a mutation-free preview (snapshot),
        // tool-first, for the package phase.
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].argv.first().map(String::as_str),
            Some("goreleaser")
        );
        assert!(requests[0].argv.contains(&"--snapshot".to_string()));
    }

    #[test]
    fn delegated_package_fails_closed_when_the_tool_produces_no_archive() {
        let root = TempDir::new().unwrap();
        // The tool exits zero but produces nothing: the declared asset is
        // missing, so normalization must fail closed rather than report a
        // phantom archive.
        let runner = FakeToolRunner::new();
        let provider =
            provider_with_delegated_package(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &runner,
            &mut reporter,
        )
        .expect_err("a delegated tool that produced no archive must fail closed");
        assert!(error.to_string().contains("did not produce"), "{error}");
    }

    #[test]
    fn delegated_package_fails_closed_on_a_stale_archive_the_tool_did_not_rewrite() {
        let root = TempDir::new().unwrap();
        let asset_rel = "dist/toven-x86_64-unknown-linux-gnu.tar.gz";
        // A stale archive from a prior run sits on disk...
        let stale = root.path().join(asset_rel);
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, b"stale-archive-bytes").unwrap();
        // ...and the tool exits 0 but writes nothing (no `with_produced_file`).
        let runner = FakeToolRunner::new();
        let provider = provider_with_delegated_package(vec![asset_rel]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &runner,
            &mut reporter,
        )
        .expect_err("a stale archive the tool did not rewrite must fail closed");
        // The stale archive was cleared before the preview, so the post-run
        // existence check fails closed instead of reusing it.
        assert!(error.to_string().contains("did not produce"), "{error}");
    }

    #[test]
    fn delegated_package_fails_closed_when_the_tool_exits_non_zero() {
        let root = TempDir::new().unwrap();
        let runner = FakeToolRunner::new()
            .with_exit_code(Some(1))
            .with_stderr("goreleaser: build failed");
        let provider =
            provider_with_delegated_package(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &runner,
            &mut reporter,
        )
        .expect_err("a non-zero delegated tool exit must fail closed");
        let message = error.to_string();
        assert!(message.contains("goreleaser"), "{message}");
        assert!(message.contains("exited 1"), "{message}");
    }

    #[test]
    fn delegated_package_runs_the_tool_preview_once_for_multiple_owned_assets() {
        let root = TempDir::new().unwrap();
        // One delegated owner declares two archives matching the target: a
        // single GoReleaser `--snapshot` produces both, so the preview must run
        // exactly once — not once per asset.
        let first = "dist/app-x86_64-unknown-linux-gnu.tar.gz";
        let second = "dist/helper-x86_64-unknown-linux-gnu.tar.gz";
        let runner = FakeToolRunner::new()
            .with_produced_file(root.path().join(first), b"app-archive")
            .with_produced_file(root.path().join(second), b"helper-archive");
        let provider = provider_with_delegated_package(vec![first, second]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &runner,
            &mut reporter,
        )
        .unwrap();

        // Both archives are normalized into the report, but the tool ran once.
        assert_eq!(report.assets.len(), 2);
        assert!(
            report
                .assets
                .iter()
                .all(|asset| asset.backing == "delegated")
        );
        assert_eq!(
            runner.requests().len(),
            1,
            "a single delegated owner's preview must run once for all its assets"
        );
    }

    #[test]
    fn produced_archive_is_byte_stable() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), LINUX, "toven");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let produced = root
            .path()
            .join("dist/toven-x86_64-unknown-linux-gnu.tar.gz");

        release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .unwrap();
        let first = std::fs::read(&produced).unwrap();
        release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .unwrap();
        let second = std::fs::read(&produced).unwrap();
        assert_eq!(
            first, second,
            "repackaging identical input must be byte-stable"
        );
    }

    #[test]
    fn packages_a_windows_zip_with_an_exe_member() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), WINDOWS, "toven.exe");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-pc-windows-msvc.zip"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            WINDOWS,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.assets.len(), 1);
        assert_eq!(report.assets[0].format, ArchiveFormat::Zip);
        let produced = root.path().join("dist/toven-x86_64-pc-windows-msvc.zip");
        let bytes = std::fs::read(&produced).unwrap();
        assert_eq!(&bytes[..2], b"PK", "a zip archive starts with the PK magic");
    }

    #[test]
    fn fails_closed_when_the_built_binary_is_missing() {
        let root = TempDir::new().unwrap();
        // No binary written.
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .expect_err("a missing built binary must fail closed");
        assert!(
            error.to_string().contains("built binary not found"),
            "{error}"
        );
    }

    #[test]
    fn fails_closed_when_no_asset_is_declared_for_the_target() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), LINUX, "toven");
        // A non-archive asset and a different-target archive: neither matches
        // the requested Linux target.
        let provider = provider_with_assets(vec![
            "dist/toven-sbom.cdx.json",
            "dist/toven-aarch64-apple-darwin.tar.gz",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .expect_err("no asset declared for the target must fail closed");
        assert!(
            error.to_string().contains("no hosted-release asset"),
            "{error}"
        );
    }

    #[test]
    fn fails_closed_when_no_assets_are_declared() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_assets(vec![]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .expect_err("no declared assets must fail closed");
        assert!(error.to_string().contains("nothing to package"), "{error}");
    }

    #[test]
    fn fails_closed_on_a_traversing_target_triple() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            "../../etc",
            None,
            &FakeToolRunner::new(),
            &mut reporter,
        )
        .expect_err("a traversing target triple must fail closed");
        assert!(
            error.to_string().contains("invalid target triple"),
            "{error}"
        );
    }
}
