//! `GitlabReleaseHost` — the GitLab [`ReleaseHost`] adapter, argv-only via
//! `glab`.
//!
//! Cuts a GitLab Release for a resolved tag by invoking the `glab` CLI as an
//! argument vector (never a shell string) through `rskit-process`, with bounded
//! output and a hard timeout. Publication is immutable create-or-verify: every
//! project-relative asset is validated before any external mutation, then a
//! Release is created with `--no-update` so an existing tag is never edited in
//! place. `glab`'s "already exists" refusal triggers a read-only verification of
//! the existing Release against the intended one; an identical Release reports
//! [`HostReleaseOutcome::AlreadyComplete`] and any divergence is a typed
//! conflict that must be forward-fixed with a new version and tag. The forge
//! token is read from the ambient environment by `glab` itself and is never
//! passed on the command line or logged.
//!
//! GitLab's release model differs from GitHub's, so this adapter does not
//! field-mirror the GitHub one:
//!
//! - **Draft.** GitLab has no draft-release concept, so a `draft` release is
//!   rejected fail-closed before any `glab` call rather than silently published
//!   as a normal Release.
//! - **Prerelease.** GitLab exposes no prerelease flag (only a future-dated
//!   `upcoming_release` marker), so `prerelease` is recorded intent the adapter
//!   cannot set on the forge. It is neither emitted nor verified — honored where
//!   a forge can represent it, dropped honestly where one cannot.
//! - **Assets.** GitLab release assets are links (`name` + `url`) with no byte
//!   size, so verification is by uploaded asset name only. `glab release view`
//!   carries no per-asset content digest or size, and this adapter never
//!   downloads remote asset bytes, so an intended asset is verified present by
//!   name and existing assets are treated as immutable — forward-fix by cutting
//!   a new version rather than replacing an asset in place.

use std::collections::BTreeSet;
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

/// Maximum retained stdout/stderr for a `glab` command (64 KiB each).
const MAX_GLAB_OUTPUT_BYTES: usize = 64 * 1024;

/// Timeout for a single `glab` command.
const GLAB_COMMAND_TIMEOUT: Duration = Duration::from_mins(2);

/// The GitLab hosted-release adapter.
#[derive(Debug, Clone, Default)]
pub struct GitlabReleaseHost;

impl GitlabReleaseHost {
    /// Construct the GitLab hosted-release adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ReleaseHost for GitlabReleaseHost {
    fn ensure_release(
        &self,
        root: &Path,
        release: &HostedRelease,
    ) -> AppResult<HostReleaseOutcome> {
        // GitLab has no draft-release concept: fail closed rather than silently
        // publish a requested draft as a normal, visible Release.
        if release.draft {
            return Err(AppError::invalid_input(
                "release.host.draft",
                format!(
                    "hosted release '{}' requests a draft, but GitLab has no draft release; \
                     remove the draft flag or cut this Release on a forge that supports drafts",
                    release.tag
                ),
            ));
        }

        // Validate every project-relative asset before any external mutation, so
        // a bad asset fails the run before it touches the forge.
        let local = validate_assets(root, release)?;

        // `--no-update` makes create refuse an existing tag instead of editing
        // it in place, so create-or-verify stays immutable even though `glab
        // release create` updates by default.
        let notes = release.notes.as_bytes();
        let created = glab(root, create_argv(release), notes)?;
        if created.success() {
            return Ok(HostReleaseOutcome::Created);
        }
        if !release_already_exists(&created) {
            // Not an idempotent re-run: surface the real `glab` failure.
            created.check()?;
        }

        // The Release already exists. Hosted publication is immutable: read the
        // existing Release and verify it matches the intended one. An identical
        // Release is an idempotent re-run; any divergence is a conflict the
        // operator forward-fixes with a new version — never an edit or a
        // clobbering re-upload.
        let viewed = glab(root, view_argv(&release.tag), &[])?;
        viewed.check()?;
        let existing = parse_existing(&viewed.stdout)?;
        reconcile(release, &local, &existing)?;
        Ok(HostReleaseOutcome::AlreadyComplete)
    }

    fn release_exists(&self, root: &Path, tag: &str) -> AppResult<bool> {
        let viewed = glab(root, view_argv(tag), &[])?;
        if viewed.success() {
            return Ok(true);
        }
        if release_not_found(&viewed) {
            return Ok(false);
        }
        // A real `glab` failure (auth, network, rate limit): surface it rather
        // than silently treating the Release as absent and creating a duplicate.
        viewed.check()?;
        Ok(false)
    }
}

/// Whether a failed `glab release view` failed because no Release exists for the
/// tag — the "Release is missing" signal the reconcile pre-pass acts on.
///
/// Matches only `glab`'s specific "release does not exist" message (emitted for
/// both a 404 and a 403 on the release), not any output containing "not found".
/// A broader match would misclassify unrelated failures — a missing project, an
/// auth error, or a rate limit — as an absent Release and drive the reconcile
/// path to create a duplicate instead of surfacing the real error.
fn release_not_found(output: &ProcessResult) -> bool {
    combined_lower(output).contains("release does not exist")
}

/// Whether a failed `glab release create --no-update` failed because the Release
/// already exists — the idempotent re-run signal.
fn release_already_exists(output: &ProcessResult) -> bool {
    combined_lower(output).contains("already exists")
}

/// The lowercased stdout+stderr of a `glab` result, for signal matching.
fn combined_lower(output: &ProcessResult) -> String {
    format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase()
}

/// Validate every asset and collect the uploaded link names GitLab will use.
///
/// Asset paths are carried project-relative through the port; this is the single
/// filesystem-touching site that resolves each one against `root` via
/// [`safe_join`], which both rejects a traversal or absolute config value and
/// yields a contained absolute path. Rejects an asset that is not a regular
/// file, and rejects two assets that would upload under the same link name. Runs
/// before any `glab` invocation so an invalid asset set never partially mutates
/// the forge.
fn validate_assets(root: &Path, release: &HostedRelease) -> AppResult<BTreeSet<String>> {
    let mut names = BTreeSet::new();
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
        let name = asset_link_name(asset)?;
        if !names.insert(name.clone()) {
            return Err(AppError::invalid_input(
                "release.host.assets",
                format!("two assets upload under the same name '{name}'"),
            ));
        }
    }
    Ok(names)
}

/// The asset-link name GitLab records for an asset: its display label when set,
/// otherwise the file name (`glab release create <path>#<label>`).
fn asset_link_name(asset: &toven_ports::ReleaseAsset) -> AppResult<String> {
    if let Some(label) = &asset.label {
        return Ok(label.clone());
    }
    asset
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset path '{}' has no file name", asset.path.display()),
            )
        })
}

/// Build the `glab release create` argv for a hosted release.
///
/// Release notes are piped through stdin via `--notes-file -`, never an argv
/// value, so changelog-derived notes cannot leak through process listings or hit
/// argv-length limits. `--no-update` keeps create-or-verify immutable: an
/// existing tag is refused rather than edited. GitLab has no draft/prerelease
/// flag, so neither is emitted (draft is rejected upstream; prerelease is
/// recorded intent only).
fn create_argv(release: &HostedRelease) -> Vec<String> {
    let mut argv = vec![
        "release".to_string(),
        "create".to_string(),
        release.tag.clone(),
        "--name".to_string(),
        release.title.clone(),
        "--notes-file".to_string(),
        "-".to_string(),
        "--no-update".to_string(),
    ];
    argv.extend(asset_args(release));
    argv
}

/// Build the read-only `glab release view` argv that fetches the existing
/// Release's metadata and assets as JSON for verification (and reuses it as the
/// existence probe).
fn view_argv(tag: &str) -> Vec<String> {
    vec![
        "release".to_string(),
        "view".to_string(),
        tag.to_string(),
        "--output".to_string(),
        "json".to_string(),
    ]
}

/// The positional asset arguments for a `glab release create` command.
///
/// A labeled asset uses `glab`'s `<path>#<label>` syntax; an unlabeled one is
/// the bare path.
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

/// An existing GitLab Release as reported by `glab release view --output json`.
#[derive(Debug, Deserialize)]
struct ExistingRelease {
    /// Release title (`name`).
    #[serde(default)]
    name: String,
    /// Release note body (`description`).
    #[serde(default)]
    description: String,
    /// Uploaded assets (link objects; name only is used).
    #[serde(default)]
    assets: ExistingAssets,
}

/// The `assets` object of a GitLab Release (only its `links` are verified).
#[derive(Debug, Default, Deserialize)]
struct ExistingAssets {
    /// Asset links attached to the Release.
    #[serde(default)]
    links: Vec<ExistingLink>,
}

/// One asset link on an existing GitLab Release.
#[derive(Debug, Deserialize)]
struct ExistingLink {
    /// Link display name.
    name: String,
}

/// Parse the JSON body of `glab release view --output json` into an
/// [`ExistingRelease`].
fn parse_existing(json: &str) -> AppResult<ExistingRelease> {
    serde_json::from_str(json.trim()).map_err(|error| {
        AppError::invalid_format("glab release view output", "release metadata JSON")
            .with_cause(error)
    })
}

/// Verify an existing Release matches the intended one, or fail with a typed
/// conflict carrying forward-fix guidance.
///
/// Compares title and notes, and — for every intended asset — that the existing
/// Release carries an identically named asset link. Extra links already present
/// on the forge (for example a separately attached signature) are tolerated; a
/// missing intended asset is a conflict. Draft/prerelease flags are not compared
/// (GitLab models neither), and asset matching is by name only (GitLab release
/// links carry no byte size or content digest) — a deliberate bound of the
/// no-download create-or-verify path, not a content-integrity guarantee.
fn reconcile(
    intended: &HostedRelease,
    local_names: &BTreeSet<String>,
    existing: &ExistingRelease,
) -> AppResult<()> {
    let mut diffs = Vec::new();
    if existing.name != intended.title {
        diffs.push(format!(
            "title (existing '{}' vs intended '{}')",
            existing.name, intended.title
        ));
    }
    if normalize_line_endings(&existing.description) != normalize_line_endings(&intended.notes) {
        diffs.push("release notes".to_string());
    }
    let remote: BTreeSet<&str> = existing
        .assets
        .links
        .iter()
        .map(|link| link.name.as_str())
        .collect();
    for name in local_names {
        if !remote.contains(name.as_str()) {
            diffs.push(format!("missing asset '{name}'"));
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

/// Normalize release-note line endings to `\n` for comparison, so a submitted
/// note body whose line endings the forge rewrote still verifies as identical on
/// an idempotent re-run.
fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn glab(root: &Path, args: Vec<String>, stdin: &[u8]) -> AppResult<ProcessResult> {
    let spec = ProcessSpec::new("glab").args(args).dir(PathBuf::from(root));
    let config = ProcessConfig::default()
        .with_timeout(Some(GLAB_COMMAND_TIMEOUT))
        .with_io(ProcessIo::captured(
            CapturedIo::new()
                .with_input(InputPolicy::Bytes(stdin.to_vec()))
                .with_output(OutputPolicy::captured().with_max_output_bytes(MAX_GLAB_OUTPUT_BYTES)),
        ));
    run(&spec, &config)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use toven_ports::{HostedRelease, ReleaseAsset, ReleaseHost};

    use super::{
        ExistingRelease, GitlabReleaseHost, ProcessResult, asset_link_name, create_argv,
        parse_existing, reconcile, release_already_exists, release_not_found, view_argv,
    };

    fn release() -> HostedRelease {
        HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "the notes")
    }

    fn failed(stderr: &str) -> ProcessResult {
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
        // `glab release view` wraps a 404/403 on the release as "release does
        // not exist." — the only absent-Release signal.
        assert!(release_not_found(&failed(
            "release does not exist.: 404 Not Found"
        )));
    }

    #[test]
    fn release_not_found_does_not_misclassify_unrelated_failures() {
        // A missing project, an auth error, or a rate limit must surface as a
        // real error — never be read as "the Release is absent", which would
        // drive the reconcile path to create a duplicate.
        for stderr in [
            "failed to fetch release.: 404 Project Not Found",
            "GET https://gitlab.example/api: 401 Unauthorized",
            "429 Too Many Requests",
        ] {
            assert!(
                !release_not_found(&failed(stderr)),
                "must not classify {stderr:?} as a missing Release"
            );
        }
    }

    #[test]
    fn release_already_exists_matches_the_no_update_refusal() {
        assert!(release_already_exists(&failed(
            "release for tag \"rust/core@1.2.3\" already exists and --no-update flag was specified"
        )));
        assert!(!release_already_exists(&failed("some other failure")));
    }

    #[test]
    fn create_argv_is_argv_only_immutable_with_assets() {
        let release = release().with_assets(vec![
            ReleaseAsset::new("dist/core.cdx.json").with_label("SBOM"),
            ReleaseAsset::new("dist/core.tgz"),
        ]);

        let argv = create_argv(&release);

        assert_eq!(&argv[0..3], &["release", "create", "rust/core@1.2.3"]);
        assert!(argv.iter().any(|arg| arg == "--name"));
        // Immutable create-or-verify: never edit an existing tag in place.
        assert!(argv.iter().any(|arg| arg == "--no-update"));
        assert!(argv.iter().any(|arg| arg == "dist/core.cdx.json#SBOM"));
        assert!(argv.iter().any(|arg| arg == "dist/core.tgz"));
        // Notes are piped through stdin (`--notes-file -`), never argv.
        assert!(argv.iter().any(|arg| arg == "--notes-file"));
        assert!(argv.iter().all(|arg| arg != "--notes"));
        assert!(argv.iter().all(|arg| arg != "the notes"));
        // GitLab has no draft/prerelease flag: neither is ever emitted.
        assert!(
            argv.iter()
                .all(|arg| arg != "--draft" && arg != "--prerelease")
        );
        // No token or shell string ever appears on the command line.
        assert!(argv.iter().all(|arg| !arg.contains("token")));
    }

    #[test]
    fn draft_release_never_reaches_create_argv() {
        // A draft is rejected in `ensure_release` before any argv is built; the
        // create argv itself carries no draft affordance to emit.
        let argv = create_argv(&release().with_draft(true));
        assert!(argv.iter().all(|arg| arg != "--draft"));
    }

    #[test]
    fn ensure_release_rejects_a_draft_before_touching_glab() {
        // The draft fail-closed gate runs first in `ensure_release`, before any
        // asset validation or `glab` spawn, so a draft never leaks out as a
        // normal visible Release. `.` is a valid root the code never reaches.
        let error = GitlabReleaseHost::new()
            .ensure_release(std::path::Path::new("."), &release().with_draft(true))
            .expect_err("a draft release must fail closed on GitLab");
        let message = error.to_string();
        assert!(message.contains("release.host.draft"), "{message}");
        assert!(message.contains("draft"), "{message}");
    }

    #[test]
    fn view_argv_is_read_only_json() {
        let argv = view_argv("rust/core@1.2.3");
        assert_eq!(&argv[0..3], &["release", "view", "rust/core@1.2.3"]);
        assert!(argv.windows(2).any(|w| w == ["--output", "json"]));
        // A read-only probe never carries a mutating verb.
        assert!(
            argv.iter()
                .all(|arg| arg != "create" && arg != "edit" && arg != "delete" && arg != "upload")
        );
    }

    #[test]
    fn asset_link_name_prefers_label_then_file_name() {
        assert_eq!(
            asset_link_name(&ReleaseAsset::new("dist/core.tgz")).expect("name"),
            "core.tgz"
        );
        assert_eq!(
            asset_link_name(&ReleaseAsset::new("dist/core.tgz").with_label("SBOM")).expect("name"),
            "SBOM"
        );
    }

    fn parsed(json: &str) -> ExistingRelease {
        parse_existing(json).expect("parse glab json")
    }

    #[test]
    fn parse_existing_maps_gitlab_field_names() {
        let existing = parsed(
            r#"{"name":"core 1.2.3","description":"the notes","tag_name":"rust/core@1.2.3",
                "assets":{"links":[{"name":"core.tgz","url":"https://x/core.tgz"}]}}"#,
        );
        assert_eq!(existing.name, "core 1.2.3");
        assert_eq!(existing.description, "the notes");
        assert_eq!(existing.assets.links.len(), 1);
        assert_eq!(existing.assets.links[0].name, "core.tgz");
    }

    #[test]
    fn reconcile_accepts_an_identical_existing_release() {
        let intended = release().with_assets(vec![ReleaseAsset::new("dist/core.tgz")]);
        let local = BTreeSet::from(["core.tgz".to_string()]);
        let existing = parsed(
            r#"{"name":"core 1.2.3","description":"the notes",
                "assets":{"links":[{"name":"core.tgz","url":"https://x/core.tgz"}]}}"#,
        );
        reconcile(&intended, &local, &existing).expect("identical release verified");
    }

    #[test]
    fn reconcile_treats_crlf_normalized_notes_as_identical() {
        let intended = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "line one\nline two\n");
        let local = BTreeSet::new();
        let existing = parsed(
            r#"{"name":"core 1.2.3","description":"line one\r\nline two\r\n","assets":{"links":[]}}"#,
        );
        reconcile(&intended, &local, &existing).expect("CRLF-normalized notes verified");
    }

    #[test]
    fn reconcile_rejects_notes_that_differ_beyond_line_endings() {
        let intended = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "line one\nline two\n");
        let local = BTreeSet::new();
        let existing =
            parsed(r#"{"name":"core 1.2.3","description":"line one\r\ndifferent\r\n","assets":{"links":[]}}"#);
        let error = reconcile(&intended, &local, &existing).expect_err("notes conflict");
        assert!(error.to_string().contains("release notes"), "{error}");
    }

    #[test]
    fn reconcile_flags_a_missing_intended_asset() {
        let intended = release().with_assets(vec![ReleaseAsset::new("dist/core.tgz")]);
        let local = BTreeSet::from(["core.tgz".to_string()]);
        let existing = parsed(r#"{"name":"core 1.2.3","description":"the notes","assets":{"links":[]}}"#);
        let error = reconcile(&intended, &local, &existing).expect_err("missing asset conflict");
        assert!(error.to_string().contains("missing asset 'core.tgz'"), "{error}");
    }

    #[test]
    fn reconcile_tolerates_extra_remote_links() {
        let intended = release().with_assets(vec![ReleaseAsset::new("dist/core.tgz")]);
        let local = BTreeSet::from(["core.tgz".to_string()]);
        let existing = parsed(
            r#"{"name":"core 1.2.3","description":"the notes","assets":{"links":[
                {"name":"core.tgz","url":"https://x/core.tgz"},
                {"name":"core.tgz.sig","url":"https://x/core.tgz.sig"}]}}"#,
        );
        reconcile(&intended, &local, &existing).expect("extra signature link tolerated");
    }
}
