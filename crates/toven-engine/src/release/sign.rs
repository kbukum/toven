//! `release sign` verb and the cosign [`Signer`] adapter.
//!
//! The engine owns signing *policy* — that only the `SHA256SUMS` manifest is
//! signed, the keyless-vs-keyed identity selection comes from
//! `[…release.sign]`, the outputs are the declared `SHA256SUMS.sig` +
//! `SHA256SUMS.pem` assets, and that a disabled or failed signer fails the
//! release closed (never an unsigned publish). The only reusable primitive is
//! "run a subprocess" ([`rskit_process`]); [`CosignSigner`] shells to the
//! runner-installed `cosign` binary argv-only, inheriting the ambient OIDC
//! identity — it embeds no signer and captures no secret.
//!
//! The verb is non-mutating: it never bumps a manifest, tags, or publishes. It
//! signs an already-produced manifest (run `release checksums` first) into the
//! declared signature/certificate asset paths.

use std::path::Path;
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::exists as file_exists;
use rskit_process::{CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_ports::{Provider, Reporter, Signer};

use super::plan::{release_targets, resolve_release_settings};
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// The manifest asset that is signed, and its signature/certificate sidecars.
const MANIFEST_NAME: &str = "SHA256SUMS";
const SIGNATURE_NAME: &str = "SHA256SUMS.sig";
const CERTIFICATE_NAME: &str = "SHA256SUMS.pem";

/// Hard bound on captured cosign output (64 KiB) — cosign is terse; this only
/// guards against a pathological stream.
const MAX_COSIGN_OUTPUT_BYTES: usize = 64 * 1024;

/// Timeout for a single cosign invocation. Keyless signing round-trips to
/// Fulcio/Rekor, so this is wider than a local command.
const COSIGN_TIMEOUT: Duration = Duration::from_mins(5);

/// A read-only projection of the signing outputs for the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SignReport {
    /// The project-relative manifest asset that was signed.
    pub blob: String,
    /// The project-relative detached-signature asset that was produced.
    pub signature: String,
    /// The project-relative signing-certificate asset that was produced.
    pub certificate: String,
}

impl SignReport {
    /// Construct a sign report.
    #[must_use]
    pub const fn new(blob: String, signature: String, certificate: String) -> Self {
        Self {
            blob,
            signature,
            certificate,
        }
    }
}

/// Sign the declared `SHA256SUMS` manifest, producing the declared
/// `SHA256SUMS.sig` + `SHA256SUMS.pem` assets via the supplied [`Signer`].
///
/// # Errors
/// Fails closed with a typed error when signing is disabled, the manifest or a
/// signature sidecar is not declared, the manifest has not been produced, or
/// the signer fails — as well as propagating configuration, discovery, graph,
/// and I/O failures.
pub fn release_sign(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    signer: &dyn Signer,
    reporter: &mut dyn Reporter,
) -> AppResult<SignReport> {
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

    let selection = match resolve_signer(&settings) {
        SignerSelection::Disabled => {
            return Err(AppError::invalid_input(
                "release.sign",
                "signing is disabled; set […release.sign].enabled = true before signing",
            ));
        }
        SignerSelection::Enabled(signer) => signer,
    };

    let declared = declared_assets(&settings);
    let blob = require_declared_asset(&declared, MANIFEST_NAME)?;
    let signature = require_declared_asset(&declared, SIGNATURE_NAME)?;
    let certificate = require_declared_asset(&declared, CERTIFICATE_NAME)?;

    let project_root = request.project_root.as_path();
    let blob_path = safe_join_asset(project_root, blob)?;
    if !file_exists(&blob_path)? {
        return Err(AppError::invalid_input(
            "release.sign.manifest",
            format!(
                "manifest '{blob}' has not been produced; run `toven release checksums` before \
                 signing"
            ),
        ));
    }
    let signature_path = safe_join_asset(project_root, signature)?;
    let certificate_path = safe_join_asset(project_root, certificate)?;
    for path in [&signature_path, &certificate_path] {
        if let Some(parent) = path.parent() {
            create_all(parent)?;
        }
    }

    signer.sign_blob(
        &blob_path,
        &signature_path,
        &certificate_path,
        selection.as_deref(),
    )?;

    Ok(SignReport::new(
        blob.clone(),
        signature.clone(),
        certificate.clone(),
    ))
}

/// The signer selection for the release scope: whether signing is enabled and,
/// if so, which key/identity ref selects it (`None` = keyless default).
enum SignerSelection {
    /// No resolved module enables signing.
    Disabled,
    /// Signing is enabled; the payload is the key ref (`None` = keyless).
    Enabled(Option<String>),
}

/// Resolve the signer selection for the release scope: `Enabled(signer)` when
/// any resolved module enables signing (`signer` is its key/identity ref,
/// `None` for the keyless default), or `Disabled` when signing is off
/// everywhere.
fn resolve_signer(
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
) -> SignerSelection {
    settings
        .values()
        .find(|resolved| resolved.sign.enabled)
        .map_or(SignerSelection::Disabled, |resolved| {
            SignerSelection::Enabled(resolved.sign.signer.clone())
        })
}

/// The sorted, de-duplicated union of every module's declared hosted-release
/// assets.
fn declared_assets(
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
) -> Vec<&String> {
    let mut assets: Vec<&String> = settings
        .values()
        .flat_map(|resolved| resolved.host.assets.iter())
        .collect();
    assets.sort();
    assets.dedup();
    assets
}

/// Find the declared asset whose file name is exactly `name`, fail-closed.
fn require_declared_asset<'a>(declared: &[&'a String], name: &str) -> AppResult<&'a String> {
    declared
        .iter()
        .copied()
        .find(|asset| asset_file_name(asset) == Some(name))
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.host.assets",
                format!(
                    "no '{name}' asset is declared; signing requires it in […release.host].assets"
                ),
            )
        })
}

/// Safe-join a declared asset onto the project root, mapping a traversal to a
/// typed error.
fn safe_join_asset(project_root: &Path, asset: &str) -> AppResult<std::path::PathBuf> {
    safe_join(project_root, asset).map_err(|error| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })
}

/// The final path component of a project-relative asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}

/// A keyless-or-keyed Sigstore [`Signer`] backed by the `cosign` binary.
///
/// Construction is stateless: the identity selection is supplied per call from
/// resolved release config. The binary is invoked argv-only through
/// [`rskit_process`]; the ambient OIDC environment the CI runner provides is
/// inherited, and no secret is placed on argv or captured.
#[derive(Debug, Clone)]
pub struct CosignSigner {
    timeout: Duration,
}

impl CosignSigner {
    /// Construct a cosign signer with the default per-invocation timeout.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout: COSIGN_TIMEOUT,
        }
    }
}

impl Default for CosignSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl Signer for CosignSigner {
    fn sign_blob(
        &self,
        blob: &Path,
        signature: &Path,
        certificate: &Path,
        signer: Option<&str>,
    ) -> AppResult<()> {
        let spec =
            ProcessSpec::new("cosign").args(cosign_argv(blob, signature, certificate, signer)?);
        let config = ProcessConfig::default()
            .with_timeout(Some(self.timeout))
            .with_io(ProcessIo::captured(CapturedIo::new().with_output(
                OutputPolicy::captured().with_max_output_bytes(MAX_COSIGN_OUTPUT_BYTES),
            )));
        run(&spec, &config)?.check()?;
        Ok(())
    }
}

/// Build the argv-only `cosign sign-blob` invocation. Keyless (no `--key`) by
/// default; a named signer selects a key ref. `--yes` skips the interactive
/// confirmation so the non-interactive CI signer never blocks.
fn cosign_argv(
    blob: &Path,
    signature: &Path,
    certificate: &Path,
    signer: Option<&str>,
) -> AppResult<Vec<String>> {
    let mut argv = vec![
        "sign-blob".to_string(),
        "--yes".to_string(),
        "--output-signature".to_string(),
        path_arg(signature)?,
        "--output-certificate".to_string(),
        path_arg(certificate)?,
    ];
    if let Some(key) = signer {
        argv.push("--key".to_string());
        argv.push(key.to_string());
    }
    argv.push(path_arg(blob)?);
    Ok(argv)
}

/// Render a path as a UTF-8 argv token, failing closed on a non-UTF-8 path
/// rather than passing a lossily-encoded argument to the signer.
fn path_arg(path: &Path) -> AppResult<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        AppError::invalid_input(
            "release.sign.path",
            format!("path '{}' is not valid UTF-8", path.display()),
        )
    })
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
        CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig, SignConfig,
        TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeSigner, RecordingReporter,
    };

    use super::{cosign_argv, release_sign};
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::plan::PlanRequest;

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

    fn sign_assets() -> Vec<&'static str> {
        vec![
            "dist/SHA256SUMS",
            "dist/SHA256SUMS.sig",
            "dist/SHA256SUMS.pem",
        ]
    }

    fn provider(sign: SignConfig, assets: Vec<&str>) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".to_string()),
                    assets: Some(assets.into_iter().map(str::to_string).collect()),
                    ..HostConfig::default()
                }),
                sign: Some(sign),
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

    fn write_manifest(root: &Path) {
        let dir = root.join("dist");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SHA256SUMS"), b"abc  toven.tar.gz\n").unwrap();
    }

    #[test]
    fn signs_the_manifest_into_the_declared_sidecars_when_enabled() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path());
        let signer_config = SignConfig {
            enabled: true,
            signer: Some("keyref".to_string()),
            ..SignConfig::default()
        };
        let provider = provider(signer_config, sign_assets());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let signer = FakeSigner::default();
        let mut reporter = RecordingReporter::new();

        let report = release_sign(
            &request(root.path()),
            &document(),
            &providers,
            &signer,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.blob, "dist/SHA256SUMS");
        assert_eq!(report.signature, "dist/SHA256SUMS.sig");
        assert_eq!(report.certificate, "dist/SHA256SUMS.pem");
        // The signer was invoked once with the configured key selection and the
        // sidecar assets exist on disk.
        assert_eq!(signer.calls().len(), 1);
        assert_eq!(signer.calls()[0].signer, Some("keyref".to_string()));
        assert!(root.path().join("dist").join("SHA256SUMS.sig").exists());
        assert!(root.path().join("dist").join("SHA256SUMS.pem").exists());
    }

    #[test]
    fn fails_closed_and_never_signs_when_signing_is_disabled() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path());
        let provider = provider(SignConfig::default(), sign_assets());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let signer = FakeSigner::default();
        let mut reporter = RecordingReporter::new();

        let error = release_sign(
            &request(root.path()),
            &document(),
            &providers,
            &signer,
            &mut reporter,
        )
        .expect_err("signing disabled must fail closed");
        assert!(error.to_string().contains("signing is disabled"));
        // The signer was never invoked — no unsigned side effect, no attempt.
        assert_eq!(signer.calls().len(), 0);
    }

    #[test]
    fn fails_closed_when_the_signer_fails() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path());
        let provider = provider(
            SignConfig {
                enabled: true,
                signer: None,
                ..SignConfig::default()
            },
            sign_assets(),
        );
        let providers: Vec<&dyn Provider> = vec![&provider];
        let signer = FakeSigner::failing("cosign is not installed");
        let mut reporter = RecordingReporter::new();

        let error = release_sign(
            &request(root.path()),
            &document(),
            &providers,
            &signer,
            &mut reporter,
        )
        .expect_err("a signer failure must abort the release");
        assert!(error.to_string().contains("cosign is not installed"));
    }

    #[test]
    fn fails_closed_when_the_manifest_has_not_been_produced() {
        let root = TempDir::new().unwrap();
        // No SHA256SUMS written to disk.
        let provider = provider(
            SignConfig {
                enabled: true,
                signer: None,
                ..SignConfig::default()
            },
            sign_assets(),
        );
        let providers: Vec<&dyn Provider> = vec![&provider];
        let signer = FakeSigner::default();
        let mut reporter = RecordingReporter::new();

        let error = release_sign(
            &request(root.path()),
            &document(),
            &providers,
            &signer,
            &mut reporter,
        )
        .expect_err("a missing manifest must fail closed");
        assert!(error.to_string().contains("has not been produced"));
        assert_eq!(signer.calls().len(), 0);
    }

    #[test]
    fn keyless_argv_omits_the_key_flag() {
        let argv = cosign_argv(
            Path::new("dist/SHA256SUMS"),
            Path::new("dist/SHA256SUMS.sig"),
            Path::new("dist/SHA256SUMS.pem"),
            None,
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "sign-blob",
                "--yes",
                "--output-signature",
                "dist/SHA256SUMS.sig",
                "--output-certificate",
                "dist/SHA256SUMS.pem",
                "dist/SHA256SUMS",
            ]
        );
    }

    #[test]
    fn keyed_argv_selects_the_key_ref() {
        let argv = cosign_argv(
            Path::new("dist/SHA256SUMS"),
            Path::new("dist/SHA256SUMS.sig"),
            Path::new("dist/SHA256SUMS.pem"),
            Some("cosign.key"),
        )
        .unwrap();
        assert!(argv.windows(2).any(|pair| pair == ["--key", "cosign.key"]));
        // The blob is the final positional argument.
        assert_eq!(argv.last().unwrap(), "dist/SHA256SUMS");
    }
}
