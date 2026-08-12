use std::path::Path;
use std::sync::Arc;

use rskit_errors::AppResult;
use toven_ports::{AssetDownloader, SignatureVerifier, ToolRunner, VersionProbe};

use super::assets::{path_arg, run_tool};

/// [`AssetDownloader`] backed by `gh release download`, driven argv-only through
/// the shared [`ToolRunner`] seam.
///
/// Authentication stays ambient (the runner's `gh` credentials inherited from
/// the parent environment); no token is placed on argv or captured.
#[derive(Clone)]
pub struct GhAssetDownloader {
    runner: Arc<dyn ToolRunner>,
}

impl GhAssetDownloader {
    /// Construct a `gh`-backed downloader driven through `runner`.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self { runner }
    }
}

impl AssetDownloader for GhAssetDownloader {
    fn download(&self, tag: &str, assets: &[&str], dest: &Path) -> AppResult<()> {
        let mut argv = vec![
            "release".to_string(),
            "download".to_string(),
            tag.to_string(),
            "--dir".to_string(),
            path_arg(dest)?,
        ];
        for asset in assets {
            argv.push("--pattern".to_string());
            argv.push((*asset).to_string());
        }
        run_tool(self.runner.as_ref(), "gh", argv, None)?;
        Ok(())
    }
}

/// [`SignatureVerifier`] backed by `cosign verify-blob`, driven argv-only
/// through the shared [`ToolRunner`] seam.
///
/// Keyless verification checks the transparency-log entry and certificate chain
/// against the configured identity/issuer. No secret is involved — verification
/// is against public Sigstore state.
#[derive(Clone)]
pub struct CosignVerifier {
    runner: Arc<dyn ToolRunner>,
}

impl CosignVerifier {
    /// Construct a cosign-backed signature verifier driven through `runner`.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self { runner }
    }
}

impl SignatureVerifier for CosignVerifier {
    fn verify_blob(
        &self,
        blob: &Path,
        signature: &Path,
        certificate: &Path,
        identity: &str,
        issuer: &str,
    ) -> AppResult<()> {
        let argv = vec![
            "verify-blob".to_string(),
            "--certificate".to_string(),
            path_arg(certificate)?,
            "--signature".to_string(),
            path_arg(signature)?,
            "--certificate-identity-regexp".to_string(),
            identity.to_string(),
            "--certificate-oidc-issuer".to_string(),
            issuer.to_string(),
            path_arg(blob)?,
        ];
        run_tool(self.runner.as_ref(), "cosign", argv, None)?;
        Ok(())
    }
}

/// [`VersionProbe`] that runs `<binary> --version` argv-only through the shared
/// [`ToolRunner`] seam and returns the stdout it prints.
#[derive(Clone)]
pub struct ProcessVersionProbe {
    runner: Arc<dyn ToolRunner>,
}

impl ProcessVersionProbe {
    /// Construct a process-backed version probe driven through `runner`.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self { runner }
    }
}

impl VersionProbe for ProcessVersionProbe {
    fn report_version(&self, binary: &Path) -> AppResult<String> {
        run_tool(
            self.runner.as_ref(),
            &path_arg(binary)?,
            vec!["--version".to_string()],
            None,
        )
    }
}
