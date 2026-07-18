//! `GithubReleaseHost` — the GitHub [`ReleaseHost`] adapter, argv-only via `gh`.
//!
//! Cuts a GitHub Release for a resolved tag by invoking the `gh` CLI as an
//! argument vector (never a shell string) through `rskit-process`, with bounded
//! output and a hard timeout. Idempotency is create-first: an "already exists"
//! response falls through to an in-place edit plus a clobbering asset upload, so
//! re-running a release updates the existing Release rather than duplicating it.
//! The forge token is read from the ambient environment by `gh` itself and is
//! never passed on the command line or logged.
//!
//! GitLab is a documented follow-up seam behind the same [`ReleaseHost`] port;
//! only GitHub is implemented here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::AppResult;
use rskit_process::{
    CapturedIo, InputPolicy, OutputPolicy, ProcessConfig, ProcessIo, ProcessResult, ProcessSpec,
    run,
};
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
        let notes = release.notes.as_bytes();
        let created = gh(root, create_argv(release), notes)?;
        if created.success() {
            return Ok(HostReleaseOutcome::Created);
        }
        if !release_already_exists(&created) {
            // Not an idempotent re-run: surface the real `gh` failure.
            created.check()?;
        }

        // The Release already exists: update it in place, then re-upload assets
        // with `--clobber`, overwriting any same-named asset instead of erroring
        // on repeats. Assets dropped from config are not deleted from the Release.
        gh(root, edit_argv(release), notes)?.check()?;
        if let Some(argv) = upload_argv(release) {
            gh(root, argv, &[])?.check()?;
        }
        Ok(HostReleaseOutcome::Updated)
    }
}

/// Whether a failed `gh release create` failed because the Release already
/// exists — the idempotent re-run signal.
fn release_already_exists(output: &ProcessResult) -> bool {
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    combined.contains("already exists")
}

/// Build the `gh release create` argv for a hosted release.
///
/// Release notes are piped through stdin via `--notes-file -`, never an argv
/// value, so changelog-derived notes cannot leak through process listings or hit
/// argv-length limits.
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

/// Build the `gh release edit` argv that reconciles an existing Release's
/// metadata (title, notes, draft/prerelease flags) with the resolved release.
///
/// Notes are piped through stdin via `--notes-file -`, matching `create_argv`.
fn edit_argv(release: &HostedRelease) -> Vec<String> {
    vec![
        "release".to_string(),
        "edit".to_string(),
        release.tag.clone(),
        "--title".to_string(),
        release.title.clone(),
        "--notes-file".to_string(),
        "-".to_string(),
        format!("--draft={}", release.draft),
        format!("--prerelease={}", release.prerelease),
    ]
}

/// Build the `gh release upload` argv, or `None` when the release has no assets.
///
/// `--clobber` overwrites an existing asset of the same name so a re-run replaces
/// it in place instead of failing on a duplicate. Assets no longer configured are
/// left on the Release; the upload adds and overwrites but never deletes.
fn upload_argv(release: &HostedRelease) -> Option<Vec<String>> {
    if release.assets.is_empty() {
        return None;
    }
    let mut argv = vec![
        "release".to_string(),
        "upload".to_string(),
        release.tag.clone(),
    ];
    argv.extend(asset_args(release));
    argv.push("--clobber".to_string());
    Some(argv)
}

/// The positional asset arguments for a `gh release` command.
///
/// A labeled asset uses `gh`'s `<path>#<label>` syntax; an unlabeled one is the
/// bare path. Both `create` and `upload` accept this form, so a clobbering
/// re-upload overwrites the same-named asset while keeping its label instead of
/// stripping it.
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

fn display(path: &Path) -> String {
    path.display().to_string()
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
    use toven_ports::{HostedRelease, ReleaseAsset};

    use super::{create_argv, edit_argv, upload_argv};

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
        // Notes are piped through stdin (`--notes-file -`), never argv, so they
        // cannot leak via process listings or hit argv-length limits.
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
    fn edit_argv_sets_explicit_flag_values() {
        let argv = edit_argv(&release().with_prerelease(true));
        assert_eq!(&argv[0..3], &["release", "edit", "rust/core@1.2.3"]);
        assert!(argv.iter().any(|arg| arg == "--draft=false"));
        assert!(argv.iter().any(|arg| arg == "--prerelease=true"));
    }

    #[test]
    fn upload_argv_is_none_without_assets_and_clobbers_with_them() {
        assert!(upload_argv(&release()).is_none());

        let with_assets = release().with_assets(vec![
            ReleaseAsset::new("dist/core.cdx.json").with_label("SBOM"),
            ReleaseAsset::new("dist/core.tgz"),
        ]);
        let argv = upload_argv(&with_assets).expect("assets present");
        assert_eq!(&argv[0..3], &["release", "upload", "rust/core@1.2.3"]);
        // The clobbering re-upload keeps the label so it does not regress to an
        // unlabeled asset.
        assert!(argv.iter().any(|arg| arg == "dist/core.cdx.json#SBOM"));
        assert!(argv.iter().any(|arg| arg == "dist/core.tgz"));
        assert!(argv.iter().any(|arg| arg == "--clobber"));
    }
}
