//! Shared doubles for the release-verification ports: [`FakeAssetDownloader`],
//! [`FakeSignatureVerifier`], and [`FakeVersionProbe`].
//!
//! These let `release verify` tests run fully offline: the downloader copies
//! assets from an in-test "remote" directory (so a test can stage a tampered
//! `SHA256SUMS` there), the signature verifier is scripted to accept or reject
//! (exercising the signature-before-checksum ordering), and the version probe
//! reports a canned version (or fails) so the version-mismatch path is covered
//! without executing a real binary.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{AssetDownloader, SignatureVerifier, VersionProbe};

/// An [`AssetDownloader`] that copies requested assets from an in-test "remote"
/// directory into the destination, recording each fetched tag.
#[derive(Debug, Clone)]
pub struct FakeAssetDownloader {
    remote: PathBuf,
    tags: Arc<Mutex<Vec<String>>>,
}

impl FakeAssetDownloader {
    /// A downloader whose "remote" is `remote`: `download` copies `remote/<name>`
    /// to `dest/<name>` for each requested asset, failing closed if absent.
    #[must_use]
    pub fn from_dir(remote: impl Into<PathBuf>) -> Self {
        Self {
            remote: remote.into(),
            tags: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The tags requested so far, in order.
    #[must_use]
    pub fn tags(&self) -> Vec<String> {
        self.tags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl AssetDownloader for FakeAssetDownloader {
    fn download(&self, tag: &str, assets: &[&str], dest: &Path) -> AppResult<()> {
        self.tags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(tag.to_string());
        for asset in assets {
            let source = self.remote.join(asset);
            let target = dest.join(asset);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    AppError::new(
                        ErrorCode::Internal,
                        format!(
                            "fake downloader cannot create '{}': {error}",
                            parent.display()
                        ),
                    )
                })?;
            }
            std::fs::copy(&source, &target).map_err(|error| {
                AppError::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "fake downloader: asset '{}' not available in remote: {error}",
                        source.display()
                    ),
                )
            })?;
        }
        Ok(())
    }
}

/// A single verification recorded by [`FakeSignatureVerifier`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VerifyCall {
    /// The blob whose signature was verified.
    pub blob: String,
    /// The keyless identity regexp the certificate was checked against.
    pub identity: String,
    /// The OIDC issuer the certificate was checked against.
    pub issuer: String,
}

/// A [`SignatureVerifier`] scripted to accept or reject, recording its calls.
#[derive(Debug, Clone, Default)]
pub struct FakeSignatureVerifier {
    inner: Arc<Mutex<VerifierState>>,
}

#[derive(Debug, Default)]
struct VerifierState {
    calls: Vec<VerifyCall>,
    fail: Option<String>,
}

impl FakeSignatureVerifier {
    /// A verifier that accepts every signature.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A verifier that rejects every signature with `message`, so the release
    /// aborts before trusting the manifest.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VerifierState {
                calls: Vec::new(),
                fail: Some(message.into()),
            })),
        }
    }

    /// The verifications recorded so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<VerifyCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, VerifierState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SignatureVerifier for FakeSignatureVerifier {
    fn verify_blob(
        &self,
        blob: &Path,
        _signature: &Path,
        _certificate: &Path,
        identity: &str,
        issuer: &str,
    ) -> AppResult<()> {
        let failure = self.state().fail.clone();
        if let Some(message) = failure {
            return Err(AppError::new(ErrorCode::Internal, message));
        }
        self.state().calls.push(VerifyCall {
            blob: blob.display().to_string(),
            identity: identity.to_string(),
            issuer: issuer.to_string(),
        });
        Ok(())
    }
}

/// A [`VersionProbe`] that reports a canned version (or fails), recording the
/// binaries it was asked to run.
#[derive(Debug, Clone)]
pub struct FakeVersionProbe {
    reported: Result<String, String>,
    probed: Arc<Mutex<Vec<String>>>,
}

impl FakeVersionProbe {
    /// A probe that reports `version` for every binary.
    #[must_use]
    pub fn reporting(version: impl Into<String>) -> Self {
        Self {
            reported: Ok(version.into()),
            probed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A probe that fails with `message` (binary missing / unreadable output).
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            reported: Err(message.into()),
            probed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The binaries probed so far, in order.
    #[must_use]
    pub fn probed(&self) -> Vec<String> {
        self.probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl VersionProbe for FakeVersionProbe {
    fn report_version(&self, binary: &Path) -> AppResult<String> {
        self.probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(binary.display().to_string());
        self.reported
            .clone()
            .map_err(|message| AppError::new(ErrorCode::Internal, message))
    }
}
