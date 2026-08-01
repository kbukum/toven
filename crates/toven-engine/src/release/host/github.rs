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
//! Asset verification is by uploaded name and byte size only: `gh release view`
//! does not return per-asset content digests, and this adapter never downloads
//! remote asset bytes, so a divergent existing asset of identical size would
//! pass verification. Published assets are therefore treated as immutable —
//! forward-fix by cutting a new version rather than replacing an asset in place.
//! (Content-digest verification would require downloading each asset or the
//! `SHA256SUMS` body and hashing it locally, which the create-or-verify path
//! deliberately does not do.)

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

    fn release_exists(&self, root: &Path, tag: &str) -> AppResult<bool> {
        let viewed = gh(root, exists_argv(tag), &[])?;
        if viewed.success() {
            return Ok(true);
        }
        if release_not_found(&viewed) {
            return Ok(false);
        }
        // A real `gh` failure (auth, network, rate limit): surface it rather
        // than silently treating the Release as absent and creating a duplicate.
        viewed.check()?;
        Ok(false)
    }
}

/// Whether a failed `gh release view` failed because no Release exists for the
/// tag — the "Release is missing" signal the reconcile pre-pass acts on.
///
/// Matches only `gh`'s specific "release not found" message, not any output
/// containing "not found". A broader match would misclassify unrelated failures
/// — a missing repository, a permission/auth error, or a rate limit — as an
/// absent Release and drive the reconcile path to create a duplicate instead of
/// surfacing the real error.
fn release_not_found(output: &ProcessResult) -> bool {
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    combined.contains("release not found")
}

/// Build the read-only `gh release view` argv that probes whether a Release
/// exists for `tag`, requesting a single minimal field.
fn exists_argv(tag: &str) -> Vec<String> {
    vec![
        "release".to_string(),
        "view".to_string(),
        tag.to_string(),
        "--json".to_string(),
        "name".to_string(),
    ]
}

/// Whether a failed `gh release create` failed because the Release already
/// exists — the idempotent re-run signal.
fn release_already_exists(output: &ProcessResult) -> bool {
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    combined.contains("already exists")
}

/// Validate and fingerprint every asset by uploaded name and byte size.
///
/// Asset paths are carried project-relative through the port; this is the
/// single filesystem-touching site that resolves each one against `root` (the
/// release/member root the assets are relative to) via
/// [`safe_join`], which both rejects a traversal or absolute config value and
/// yields a contained absolute path. Rejects an asset that is not a regular
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
///
/// Asset matching is name + byte size only (the limitation documented on the
/// module): `gh release view` carries no content digest, so a same-size but
/// byte-divergent asset is not detected here. This is a deliberate bound of the
/// no-download create-or-verify path, not a content-integrity guarantee.
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

    use rskit_fs::TempDir;
    use toven_ports::{HostedRelease, ReleaseAsset};

    use super::{
        ExistingRelease, ProcessResult, create_argv, exists_argv, fingerprint_assets,
        parse_existing, reconcile, release_not_found, view_argv,
    };

    fn release() -> HostedRelease {
        HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "the notes")
    }

    fn failed_view(stderr: &str) -> ProcessResult {
        ProcessResult::completed(
            Some(1),
            Vec::new(),
            stderr.as_bytes().to_vec(),
            false,
            false,
            std::time::Duration::from_millis(0),
            false,
            false,
        )
    }

    #[test]
    fn release_not_found_matches_only_the_release_missing_signal() {
        // The specific `gh release view` "not found" message is the only
        // absent-Release signal.
        assert!(release_not_found(&failed_view("release not found")));
        assert!(release_not_found(&failed_view(
            "HTTP 404: Release not found"
        )));
    }

    #[test]
    fn release_not_found_does_not_misclassify_unrelated_failures() {
        // A missing repo, an auth/permission error, or a rate limit must surface
        // as a real error — never be read as "the Release is absent", which would
        // drive the reconcile path to create a duplicate.
        for stderr in [
            "could not resolve to a Repository with the name 'kbukum/nope'. (not found)",
            "GraphQL: Could not resolve to a Repository (repository not found)",
            "HTTP 403: Resource not accessible by integration",
            "API rate limit exceeded",
        ] {
            assert!(
                !release_not_found(&failed_view(stderr)),
                "must not classify {stderr:?} as a missing Release"
            );
        }
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

    #[test]
    fn exists_argv_is_a_read_only_probe() {
        let argv = exists_argv("rust/core@0.1.0-alpha.1");
        assert_eq!(&argv[0..3], &["release", "view", "rust/core@0.1.0-alpha.1"]);
        assert!(argv.iter().any(|arg| arg == "--json"));
        // A read-only existence probe never carries a mutating verb.
        assert!(
            argv.iter()
                .all(|arg| arg != "create" && arg != "edit" && arg != "upload" && arg != "delete")
        );
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

    #[test]
    fn fingerprint_assets_maps_a_relative_asset_to_name_and_size() {
        // The producer keeps asset paths project-relative; fingerprinting is the
        // single fs-touching site that resolves them against the release root.
        let temp = TempDir::new().expect("temp dir");
        temp.write_file("dist/app.tgz", b"payload-bytes")
            .expect("stage asset");
        let release = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "notes")
            .with_assets(vec![ReleaseAsset::new("dist/app.tgz")]);

        let sizes =
            fingerprint_assets(temp.path(), &release).expect("relative asset fingerprinted");

        assert_eq!(sizes, BTreeMap::from([("app.tgz".to_string(), 13_u64)]));
    }

    #[test]
    fn fingerprint_assets_rejects_a_traversal_asset_path() {
        let temp = TempDir::new().expect("temp dir");
        let release = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "notes")
            .with_assets(vec![ReleaseAsset::new("../evil")]);

        let error = fingerprint_assets(temp.path(), &release)
            .expect_err("a traversal asset path fails closed");

        let message = error.to_string();
        assert!(message.contains("release.host.assets"), "{message}");
        assert!(message.contains("../evil"), "{message}");
        assert!(message.contains("escapes the repository root"), "{message}");
    }

    #[test]
    fn fingerprint_assets_rejects_two_assets_uploading_under_the_same_name() {
        let temp = TempDir::new().expect("temp dir");
        temp.write_file("a/app.tgz", b"one").expect("stage a");
        temp.write_file("b/app.tgz", b"two").expect("stage b");
        let release =
            HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "notes").with_assets(vec![
                ReleaseAsset::new("a/app.tgz"),
                ReleaseAsset::new("b/app.tgz"),
            ]);

        let error = fingerprint_assets(temp.path(), &release)
            .expect_err("colliding upload names fail closed");

        assert!(error.to_string().contains("same name 'app.tgz'"), "{error}");
    }
}
