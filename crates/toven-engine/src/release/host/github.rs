//! `GithubReleaseHost` — the GitHub [`ReleaseHost`] adapter, argv-only via
//! `gh`.
//!
//! Cuts a GitHub Release for a resolved tag by invoking the `gh` CLI as an
//! argument vector (never a shell string) through `rskit-process`, with bounded
//! output and a hard timeout. Publication is immutable create-or-verify: every
//! project-relative asset is validated and fingerprinted before any external
//! mutation, then a Release is created; an "already exists" response triggers a
//! read-only verification of the existing Release against the intended one
//! rather than an in-place edit. An identical Release reports
//! [`HostReleaseOutcome::AlreadyComplete`]; any divergence is a typed conflict
//! that must be forward-fixed with a new version and tag. The forge token is
//! read from the ambient environment by `gh` itself and is never passed on the
//! command line or logged.
//!
//! GitLab is a documented follow-up seam behind the same [`ReleaseHost`] port;
//! only GitHub is implemented here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::path::safe_join;
use rskit_fs::sync_io::file;
use rskit_process::{
    CapturedIo, InputPolicy, OutputPolicy, ProcessConfig, ProcessIo, ProcessResult, ProcessSpec,
    run,
};
use serde::Deserialize;
use toven_ports::{HostReleaseOutcome, HostedRelease, ReleaseHost};

/// Maximum retained stdout/stderr for a `gh` command (64 KiB each).
const MAX_GH_OUTPUT_BYTES: usize = 64 * 1024;

/// Timeout for a single `gh` command.
const GH_COMMAND_TIMEOUT: Duration = Duration::from_mins(2);

/// The GitHub hosted-release adapter.
#[derive(Debug, Clone, Default)]
pub struct GithubReleaseHost;

impl GithubReleaseHost {
    /// Construct the GitHub hosted-release adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ReleaseHost for GithubReleaseHost {
    fn ensure_release(
        &self,
        root: &Path,
        release: &HostedRelease,
    ) -> AppResult<HostReleaseOutcome> {
        // Validate and fingerprint every project-relative asset before any
        // external mutation, so a bad asset fails the run before it touches the
        // forge.
        let local = fingerprint_assets(root, release)?;

        let notes = release.notes.as_bytes();
        let created = gh(root, create_argv(release), notes)?;
        if created.success() {
            return Ok(HostReleaseOutcome::Created);
        }
        if !release_already_exists(&created) {
            // Not an idempotent re-run: surface the real `gh` failure.
            created.check()?;
        }

        // The Release already exists. Hosted publication is immutable: read the
        // existing Release and verify it matches the intended one exactly. An
        // identical Release is an idempotent re-run; any divergence is a
        // conflict the operator forward-fixes with a new version — never an edit
        // or a clobbering re-upload.
        let viewed = gh(root, view_argv(&release.tag), &[])?;
        viewed.check()?;
        let existing = parse_existing(&viewed.stdout)?;
        reconcile(release, &local, &existing)?;
        Ok(HostReleaseOutcome::AlreadyComplete)
    }
}

/// Whether a failed `gh release create` failed because the Release already
/// exists — the idempotent re-run signal.
fn release_already_exists(output: &ProcessResult) -> bool {
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    combined.contains("already exists")
}

/// Validate and fingerprint every asset by uploaded name and byte size.
///
/// Rejects an asset path that escapes the repository root or is not a regular
/// file, and rejects two assets that would upload under the same name. Runs
/// before any `gh` invocation so an invalid asset set never partially mutates
/// the forge.
fn fingerprint_assets(root: &Path, release: &HostedRelease) -> AppResult<BTreeMap<String, u64>> {
    let mut sizes = BTreeMap::new();
    for asset in &release.assets {
        let joined = safe_join(root, &asset.path).map_err(|error| {
            AppError::invalid_input(
                "release.host.assets",
                format!(
                    "asset path '{}' escapes the repository root",
                    asset.path.display()
                ),
            )
            .with_cause(error)
        })?;
        let meta = file::metadata(&joined)?;
        if !meta.is_file {
            return Err(AppError::invalid_input(
                "release.host.assets",
                format!("asset '{}' is not a regular file", asset.path.display()),
            ));
        }
        let name = asset_name(&asset.path)?;
        if sizes.insert(name.clone(), meta.len).is_some() {
            return Err(AppError::invalid_input(
                "release.host.assets",
                format!("two assets upload under the same name '{name}'"),
            ));
        }
    }
    Ok(sizes)
}

/// The uploaded asset name GitHub derives from a path (its file name).
fn asset_name(path: &Path) -> AppResult<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset path '{}' has no file name", path.display()),
            )
        })
}

/// Build the `gh release create` argv for a hosted release.
///
/// Release notes are piped through stdin via `--notes-file -`, never an argv
/// value, so changelog-derived notes cannot leak through process listings or
/// hit argv-length limits.
fn create_argv(release: &HostedRelease) -> Vec<String> {
    let mut argv = vec![
        "release".to_string(),
        "create".to_string(),
        release.tag.clone(),
        "--title".to_string(),
        release.title.clone(),
        "--notes-file".to_string(),
        "-".to_string(),
    ];
    if release.draft {
        argv.push("--draft".to_string());
    }
    if release.prerelease {
        argv.push("--prerelease".to_string());
    }
    argv.extend(asset_args(release));
    argv
}

/// Build the read-only `gh release view` argv that fetches the existing
/// Release's metadata and assets as JSON for verification.
fn view_argv(tag: &str) -> Vec<String> {
    vec![
        "release".to_string(),
        "view".to_string(),
        tag.to_string(),
        "--json".to_string(),
        "name,body,isDraft,isPrerelease,assets".to_string(),
    ]
}

/// The positional asset arguments for a `gh release create` command.
///
/// A labeled asset uses `gh`'s `<path>#<label>` syntax; an unlabeled one is the
/// bare path.
fn asset_args(release: &HostedRelease) -> Vec<String> {
    release
        .assets
        .iter()
        .map(|asset| {
            asset.label.as_ref().map_or_else(
                || display(&asset.path),
                |label| format!("{}#{label}", display(&asset.path)),
            )
        })
        .collect()
}

/// An existing forge Release as reported by `gh release view --json`.
#[derive(Debug, Deserialize)]
struct ExistingRelease {
    /// Release title (`gh`'s `name`).
    #[serde(default)]
    name: String,
    /// Release note body.
    #[serde(default)]
    body: String,
    /// Whether the Release is a draft.
    #[serde(rename = "isDraft", default)]
    draft: bool,
    /// Whether the Release is a prerelease.
    #[serde(rename = "isPrerelease", default)]
    prerelease: bool,
    /// Uploaded assets (name and byte size).
    #[serde(default)]
    assets: Vec<ExistingAsset>,
}

/// One asset on an existing forge Release.
#[derive(Debug, Deserialize)]
struct ExistingAsset {
    /// Uploaded asset name.
    name: String,
    /// Asset size in bytes.
    #[serde(default)]
    size: u64,
}

/// Parse the JSON body of `gh release view --json` into an [`ExistingRelease`].
fn parse_existing(json: &str) -> AppResult<ExistingRelease> {
    serde_json::from_str(json.trim()).map_err(|error| {
        AppError::invalid_format("gh release view output", "release metadata JSON")
            .with_cause(error)
    })
}

/// Verify an existing Release matches the intended one exactly, or fail with a
/// typed conflict carrying forward-fix guidance.
///
/// Compares title, notes, the draft/prerelease flags, and — for every intended
/// asset — that the existing Release carries an identically named asset of the
/// same byte size. Extra assets already present on the forge (for example a
/// separately attached signature) are tolerated; a missing or size-mismatched
/// intended asset is a conflict.
fn reconcile(
    intended: &HostedRelease,
    local_sizes: &BTreeMap<String, u64>,
    existing: &ExistingRelease,
) -> AppResult<()> {
    let mut diffs = Vec::new();
    if existing.name != intended.title {
        diffs.push(format!(
            "title (existing '{}' vs intended '{}')",
            existing.name, intended.title
        ));
    }
    if normalize_line_endings(&existing.body) != normalize_line_endings(&intended.notes) {
        diffs.push("release notes".to_string());
    }
    if existing.draft != intended.draft {
        diffs.push(format!(
            "draft flag (existing {} vs intended {})",
            existing.draft, intended.draft
        ));
    }
    if existing.prerelease != intended.prerelease {
        diffs.push(format!(
            "prerelease flag (existing {} vs intended {})",
            existing.prerelease, intended.prerelease
        ));
    }
    let remote: BTreeMap<&str, u64> = existing
        .assets
        .iter()
        .map(|asset| (asset.name.as_str(), asset.size))
        .collect();
    for (name, size) in local_sizes {
        match remote.get(name.as_str()) {
            None => diffs.push(format!("missing asset '{name}'")),
            Some(remote_size) if remote_size != size => diffs.push(format!(
                "asset '{name}' size (existing {remote_size} vs intended {size})"
            )),
            Some(_) => {}
        }
    }
    if diffs.is_empty() {
        return Ok(());
    }
    Err(AppError::new(
        ErrorCode::Conflict,
        format!(
            "hosted release '{}' already exists and differs from the intended release ({}); \
             hosted releases are immutable — forward-fix by cutting a new version and tag rather \
             than editing the existing release",
            intended.tag,
            diffs.join("; ")
        ),
    ))
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// Normalize release-note line endings to `\n` for comparison.
///
/// The forge normalizes a submitted note body's line endings server-side (LF is
/// returned as CRLF by `gh release view --json body`), so an idempotent re-run
/// of a byte-identical multi-line release would otherwise report a spurious
/// notes conflict. Comparing on a canonical `\n` form keeps create-or-verify
/// idempotent without weakening the immutability check.
fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn gh(root: &Path, args: Vec<String>, stdin: &[u8]) -> AppResult<ProcessResult> {
    let spec = ProcessSpec::new("gh").args(args).dir(PathBuf::from(root));
    let config = ProcessConfig::default()
        .with_timeout(Some(GH_COMMAND_TIMEOUT))
        .with_io(ProcessIo::captured(
            CapturedIo::new()
                .with_input(InputPolicy::Bytes(stdin.to_vec()))
                .with_output(OutputPolicy::captured().with_max_output_bytes(MAX_GH_OUTPUT_BYTES)),
        ));
    run(&spec, &config)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_ports::{HostedRelease, ReleaseAsset};

    use super::{ExistingRelease, create_argv, parse_existing, reconcile, view_argv};

    fn release() -> HostedRelease {
        HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "the notes")
    }

    #[test]
    fn create_argv_is_argv_only_with_flags_and_assets() {
        let release = release()
            .with_draft(true)
            .with_prerelease(true)
            .with_assets(vec![
                ReleaseAsset::new("dist/core.cdx.json").with_label("SBOM"),
                ReleaseAsset::new("dist/core.tgz"),
            ]);

        let argv = create_argv(&release);

        assert_eq!(&argv[0..3], &["release", "create", "rust/core@1.2.3"]);
        assert!(argv.iter().any(|arg| arg == "--title"));
        assert!(argv.iter().any(|arg| arg == "--draft"));
        assert!(argv.iter().any(|arg| arg == "--prerelease"));
        assert!(argv.iter().any(|arg| arg == "dist/core.cdx.json#SBOM"));
        assert!(argv.iter().any(|arg| arg == "dist/core.tgz"));
        // Notes are piped through stdin (`--notes-file -`), never argv, so they cannot
        // leak via process listings or hit argv-length limits.
        assert!(argv.iter().any(|arg| arg == "--notes-file"));
        assert!(argv.iter().all(|arg| arg != "--notes"));
        assert!(argv.iter().all(|arg| arg != "the notes"));
        // No token or shell string ever appears on the command line.
        assert!(argv.iter().all(|arg| !arg.contains("token")));
    }

    #[test]
    fn create_argv_omits_flags_when_unset() {
        let argv = create_argv(&release());
        assert!(!argv.iter().any(|arg| arg == "--draft"));
        assert!(!argv.iter().any(|arg| arg == "--prerelease"));
    }

    #[test]
    fn view_argv_is_read_only_json() {
        let argv = view_argv("rust/core@1.2.3");
        assert_eq!(&argv[0..3], &["release", "view", "rust/core@1.2.3"]);
        assert!(argv.iter().any(|arg| arg == "--json"));
        // No mutating verb ever appears.
        assert!(argv.iter().all(|arg| arg != "edit" && arg != "upload"));
    }

    fn parsed(json: &str) -> ExistingRelease {
        parse_existing(json).expect("parse gh json")
    }

    #[test]
    fn parse_existing_maps_gh_field_names() {
        let existing = parsed(
            r#"{"name":"core 1.2.3","body":"the notes","isDraft":false,
                "isPrerelease":true,"assets":[{"name":"core.tgz","size":42}]}"#,
        );
        assert_eq!(existing.name, "core 1.2.3");
        assert_eq!(existing.body, "the notes");
        assert!(!existing.draft);
        assert!(existing.prerelease);
        assert_eq!(existing.assets.len(), 1);
        assert_eq!(existing.assets[0].name, "core.tgz");
        assert_eq!(existing.assets[0].size, 42);
    }

    #[test]
    fn reconcile_accepts_a_byte_identical_existing_release() {
        let intended = release().with_assets(vec![ReleaseAsset::new("dist/core.tgz")]);
        let local = BTreeMap::from([("core.tgz".to_string(), 42_u64)]);
        let existing = parsed(
            r#"{"name":"core 1.2.3","body":"the notes","isDraft":false,
                "isPrerelease":false,"assets":[{"name":"core.tgz","size":42}]}"#,
        );
        reconcile(&intended, &local, &existing).expect("identical release verified");
    }

    #[test]
    fn reconcile_treats_crlf_normalized_notes_as_identical() {
        // The forge returns a submitted LF note body with CRLF line endings, so
        // an idempotent re-run must still verify as complete rather than report a
        // spurious notes conflict.
        let intended = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "line one\nline two\n");
        let local = BTreeMap::new();
        let existing = parsed(
            r#"{"name":"core 1.2.3","body":"line one\r\nline two\r\n","isDraft":false,
                "isPrerelease":false,"assets":[]}"#,
        );
        reconcile(&intended, &local, &existing).expect("CRLF-normalized notes verified");
    }

    #[test]
    fn reconcile_rejects_notes_that_differ_beyond_line_endings() {
        let intended = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "line one\nline two\n");
        let local = BTreeMap::new();
        let existing = parsed(
            r#"{"name":"core 1.2.3","body":"line one\r\ndifferent\r\n","isDraft":false,
                "isPrerelease":false,"assets":[]}"#,
        );
        let error = reconcile(&intended, &local, &existing).expect_err("notes conflict");
        assert!(error.to_string().contains("release notes"), "{error}");
    }

    #[test]
    fn reconcile_tolerates_extra_remote_assets() {
        let intended = release().with_assets(vec![ReleaseAsset::new("dist/core.tgz")]);
        let local = BTreeMap::from([("core.tgz".to_string(), 42_u64)]);
        let existing = parsed(
            r#"{"name":"core 1.2.3","body":"the notes","isDraft":false,"isPrerelease":false,
                "assets":[{"name":"core.tgz","size":42},{"name":"core.tgz.sig","size":9}]}"#,
        );
        reconcile(&intended, &local, &existing).expect("extra signature asset tolerated");
    }

    #[test]
    fn reconcile_rejects_conflicting_metadata_with_forward_fix_guidance() {
        let intended = release();
        let local = BTreeMap::new();
        let existing = parsed(
            r#"{"name":"different title","body":"the notes","isDraft":false,"isPrerelease":false,"assets":[]}"#,
        );
        let error = reconcile(&intended, &local, &existing).expect_err("metadata conflict");
        let message = error.to_string();
        assert!(message.contains("title"), "{message}");
        assert!(message.contains("immutable"), "{message}");
        assert!(message.contains("forward-fix"), "{message}");
    }

    #[test]
    fn reconcile_rejects_a_missing_or_resized_asset() {
        let intended = release().with_assets(vec![ReleaseAsset::new("dist/core.tgz")]);
        let local = BTreeMap::from([("core.tgz".to_string(), 42_u64)]);

        let missing = parsed(
            r#"{"name":"core 1.2.3","body":"the notes","isDraft":false,"isPrerelease":false,"assets":[]}"#,
        );
        assert!(
            reconcile(&intended, &local, &missing)
                .expect_err("missing asset")
                .to_string()
                .contains("missing asset 'core.tgz'")
        );

        let resized = parsed(
            r#"{"name":"core 1.2.3","body":"the notes","isDraft":false,"isPrerelease":false,
                "assets":[{"name":"core.tgz","size":7}]}"#,
        );
        assert!(
            reconcile(&intended, &local, &resized)
                .expect_err("resized asset")
                .to_string()
                .contains("size")
        );
    }
}
