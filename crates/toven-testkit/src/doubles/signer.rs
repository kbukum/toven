//! Shared [`Signer`] double: [`FakeSigner`].
//!
//! Release-engine tests configure signing outcomes and call recording here
//! instead of shelling to a real `cosign` binary. It writes deterministic
//! signature/certificate bytes to the requested paths so callers can assert the
//! sidecar assets exist, records the identity selection it was invoked with,
//! and can be scripted to fail so the fail-closed path is exercised offline. It
//! is `Clone` (shared state) so a test can hold a handle for assertions after
//! injecting it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::Signer;

/// A single call recorded by [`FakeSigner`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SignerCall {
    /// The blob that was signed.
    pub blob: String,
    /// The signature output path.
    pub signature: String,
    /// The certificate output path.
    pub certificate: String,
    /// The identity/key selection (`None` = keyless default).
    pub signer: Option<String>,
}

#[derive(Debug, Default)]
struct FakeSignerState {
    calls: Vec<SignerCall>,
    fail: Option<String>,
}

/// A [`Signer`] that writes deterministic sidecars and records its calls, or
/// fails when scripted to.
#[derive(Debug, Clone, Default)]
pub struct FakeSigner {
    inner: Arc<Mutex<FakeSignerState>>,
}

impl FakeSigner {
    /// A signer that succeeds, writing deterministic signature/certificate bytes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A signer that always fails with `message` (cosign missing / signing
    /// error), so the release aborts before publishing anything unsigned.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeSignerState {
                calls: Vec::new(),
                fail: Some(message.into()),
            })),
        }
    }

    /// The calls recorded so far, in invocation order.
    #[must_use]
    pub fn calls(&self) -> Vec<SignerCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeSignerState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Signer for FakeSigner {
    fn sign_blob(
        &self,
        blob: &Path,
        signature: &Path,
        certificate: &Path,
        signer: Option<&str>,
    ) -> AppResult<()> {
        let failure = self.state().fail.clone();
        if let Some(message) = failure {
            return Err(AppError::new(ErrorCode::Internal, message));
        }
        std::fs::write(signature, b"fake-signature\n").map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "fake signer cannot write '{}': {error}",
                    signature.display()
                ),
            )
        })?;
        std::fs::write(certificate, b"fake-certificate\n").map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "fake signer cannot write '{}': {error}",
                    certificate.display()
                ),
            )
        })?;
        self.state().calls.push(SignerCall {
            blob: blob.display().to_string(),
            signature: signature.display().to_string(),
            certificate: certificate.display().to_string(),
            signer: signer.map(str::to_string),
        });
        Ok(())
    }
}
