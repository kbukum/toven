//! Signature-verification port: verify a detached Sigstore signature over a
//! release blob against a keyless identity.
//!
//! Verification *policy* — which blob is verified, the fail-closed ordering
//! (signature before checksum before extract), and the identity/issuer the
//! certificate must match — is release-engine domain. This port is the thin
//! reusable sliver: verify this signature over this blob. The keyless identity
//! (`certificate-identity-regexp`) and issuer (`certificate-oidc-issuer`) are
//! passed per call from resolved config, so any consumer verifies against their
//! own workflow identity; no secret is involved (keyless verification checks a
//! public transparency-log entry and certificate chain).

use std::path::Path;

use rskit_errors::AppResult;

/// Verifies a detached keyless signature over a blob (e.g. Sigstore cosign
/// `verify-blob`).
pub trait SignatureVerifier: Send + Sync {
    /// Verify that `signature` (with signing `certificate`) is a valid Sigstore
    /// signature over `blob`, issued to a certificate whose identity matches
    /// `identity` and whose OIDC issuer is `issuer`.
    ///
    /// # Errors
    /// Fails closed when the signature is invalid, the identity/issuer does not
    /// match, or the verifier binary is absent, so a tampered or foreign-signed
    /// manifest is never trusted.
    fn verify_blob(
        &self,
        blob: &Path,
        signature: &Path,
        certificate: &Path,
        identity: &str,
        issuer: &str,
    ) -> AppResult<()>;
}
