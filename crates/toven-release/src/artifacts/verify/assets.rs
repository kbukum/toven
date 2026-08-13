use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::archive::{ExtractLimits, extract_tar_gz, extract_zip};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::open;
use rskit_util::hash::sha256::sha256_reader;
use rskit_version::semver::Version;
use toven_ports::{ToolInvocation, ToolRunner};

use crate::model::settings::ResolvedReleaseSettings;

/// The signed manifest and its Sigstore signature/certificate sidecars.
pub(super) const MANIFEST_NAME: &str = "SHA256SUMS";
pub(super) const SIGNATURE_NAME: &str = "SHA256SUMS.sig";
pub(super) const CERTIFICATE_NAME: &str = "SHA256SUMS.pem";

/// The two archive extensions a declared asset can carry.
const TAR_GZ_EXT: &str = ".tar.gz";
const ZIP_EXT: &str = ".zip";

/// Timeout for a single `gh`/`cosign`/binary invocation. Downloads and keyless
/// verification round-trip to the forge and Fulcio/Rekor, so this is wider than
/// a local command.
const VERIFY_TIMEOUT: Duration = Duration::from_mins(5);

/// Hard bound on captured tool output (256 KiB) — guards against a pathological
/// stream while leaving room for a download progress log.
const MAX_VERIFY_OUTPUT_BYTES: usize = 256 * 1024;

/// Extract the archive at `archive` into `dest` and return the single packaged
/// binary member.
pub(super) fn extract_binary(archive: &Path, dest: &Path) -> AppResult<PathBuf> {
    let name = asset_file_name_path(archive)?;
    let extracted = if name.ends_with(ZIP_EXT) {
        extract_zip(archive, dest, ExtractLimits::default())?
    } else if name.ends_with(TAR_GZ_EXT) {
        extract_tar_gz(archive, dest, ExtractLimits::default())?
    } else {
        return Err(AppError::invalid_input(
            "release.verify.archive",
            format!(
                "archive '{}' is neither a .tar.gz nor a .zip",
                archive.display()
            ),
        ));
    };
    // The release contract is exactly one directly runnable binary per archive;
    // more than one extracted member makes the verification target ambiguous, so
    // fail closed rather than silently running the first.
    let mut members = extracted.into_iter();
    match (members.next(), members.next()) {
        (None, _) => Err(AppError::invalid_input(
            "release.verify.archive",
            format!(
                "archive '{}' contained no packaged binary",
                archive.display()
            ),
        )),
        (Some(binary), None) => Ok(binary),
        (Some(_), Some(_)) => Err(AppError::invalid_input(
            "release.verify.archive",
            format!(
                "archive '{}' contained more than one member; expected exactly one \
                 runnable binary",
                archive.display()
            ),
        )),
    }
}

/// The binary's program name for the expected version line: its file name with
/// any `.exe` extension stripped.
pub(super) fn binary_stem(binary: &Path) -> AppResult<String> {
    let name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("extracted binary '{}' has no file name", binary.display()),
            )
        })?;
    Ok(name.strip_suffix(".exe").unwrap_or(name).to_string())
}

/// Decide the single version every releasable module must report. The locked
/// same-version-per-kit policy means every module declares the same version; a
/// disagreement is a fail-closed error rather than a silent pick.
pub(super) fn decide_version(
    context: &toven_core::plan::PlanContext,
    targets: &crate::ReleaseTargets,
    settings: &BTreeMap<toven_model::ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<Version> {
    let mut decided: Option<Version> = None;
    for module in &context.federation.modules {
        let key = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&key) else {
            continue;
        };
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        if !resolved.publication.releases() {
            continue;
        }
        let declared = target.declared_version(module)?;
        match &decided {
            None => decided = Some(declared),
            Some(existing) if existing != &declared => {
                return Err(AppError::invalid_input(
                    "release.verify.version",
                    format!(
                        "releasable modules declare divergent versions ({existing} vs \
                         {declared}); cannot decide a single expected version"
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    decided.ok_or_else(|| {
        AppError::invalid_input(
            "release.verify",
            "no releasable module declares a version to verify against",
        )
    })
}

/// The declared assets that are archives (`.tar.gz` / `.zip`), in declared
/// order.
pub(super) fn archive_assets<'a>(declared: &[&'a String]) -> Vec<&'a String> {
    declared
        .iter()
        .filter(|asset| {
            asset_file_name(asset)
                .is_ok_and(|name| name.ends_with(TAR_GZ_EXT) || name.ends_with(ZIP_EXT))
        })
        .copied()
        .collect()
}

/// Parse a `SHA256SUMS` body (`shasum -a 256` two-space format) into a
/// name → lowercase-hex map.
pub(super) fn parse_manifest(path: &Path) -> AppResult<BTreeMap<String, String>> {
    let bytes = rskit_fs::sync_io::file::read(path)?;
    let text = String::from_utf8(bytes).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidInput,
            format!("manifest '{}' is not valid UTF-8: {error}", path.display()),
        )
    })?;
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (hex, name) = line.split_once("  ").ok_or_else(|| {
            AppError::invalid_input(
                "release.verify.checksum",
                format!("malformed manifest line '{line}' (expected '<hex>  <name>')"),
            )
        })?;
        entries.insert(name.to_string(), hex.to_ascii_lowercase());
    }
    Ok(entries)
}

/// The lowercase-hex SHA-256 digest of the file at `path`.
pub(super) fn digest_hex(path: &Path) -> AppResult<String> {
    let mut file = open(path).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("cannot open '{}' for checksum: {error}", path.display()),
        )
        .with_cause(error)
    })?;
    Ok(sha256_reader(&mut file)?.to_hex())
}

/// Require a configured keyless verification field (identity/issuer).
pub(super) fn require_identity<'a>(
    _settings: &ResolvedReleaseSettings,
    field: &str,
    value: Option<&'a str>,
) -> AppResult<&'a str> {
    value.ok_or_else(|| {
        AppError::invalid_input(
            format!("release.sign.{field}"),
            format!("download verification needs the keyless {field}; set […release.sign].{field}"),
        )
    })
}

/// Build the release tag from the configured tag format (default `v{version}`)
/// by substituting the decided version.
#[allow(clippy::literal_string_with_formatting_args)]
pub(super) fn build_tag(tag_format: Option<&str>, version: &Version) -> String {
    tag_format
        .unwrap_or("v{version}")
        .replace("{version}", &version.to_string())
}

/// Resolve a declared project-relative asset to an absolute path, mapping a
/// traversing path to a typed error.
pub(super) fn safe_join_asset(project_root: &Path, asset: &str) -> AppResult<PathBuf> {
    safe_join(project_root, asset).map_err(|error| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })
}

/// The final path component of a project-relative asset path.
pub(super) fn asset_file_name(asset: &str) -> AppResult<&str> {
    Path::new(asset)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset '{asset}' has no file name"),
            )
        })
}

/// The final path component of an on-disk path as a string.
pub(super) fn asset_file_name_path(path: &Path) -> AppResult<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("path '{}' has no file name", path.display()),
            )
        })
}

/// Run an argv-only external tool through the shared [`ToolRunner`] seam with a
/// bounded, captured output and the shared verify timeout, mapping a
/// spawn/exec/non-zero failure to a typed error and returning captured stdout.
pub(super) fn run_tool(
    runner: &dyn ToolRunner,
    program: &str,
    argv: Vec<String>,
    cwd: Option<&Path>,
) -> AppResult<String> {
    let mut full_argv = Vec::with_capacity(argv.len() + 1);
    full_argv.push(program.to_string());
    full_argv.extend(argv);
    let mut invocation = ToolInvocation::new(full_argv)
        .with_timeout(VERIFY_TIMEOUT)
        .with_max_output_bytes(MAX_VERIFY_OUTPUT_BYTES);
    if let Some(cwd) = cwd {
        invocation = invocation.with_working_dir(cwd);
    }
    let outcome = runner.run(&invocation)?;
    outcome.require_success(&format!("verify tool `{program}`"))?;
    Ok(outcome.stdout)
}

/// Render a path as a UTF-8 argv string, failing closed on a non-UTF-8 path so
/// nothing lossy reaches an external tool.
pub(super) fn path_arg(path: &Path) -> AppResult<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!("path '{}' is not valid UTF-8", path.display()),
        )
    })
}
