//! Signature-verification port: verify a Sigstore bundle over a release blob
//! against a keyless identity.
//!
//! Verification *policy* — which blob is verified, the fail-closed ordering
//! (signature before checksum before extract), and the identity/issuer the
//! certificate must match — is release-engine domain. This port is the thin
//! reusable sliver: verify this bundle over this blob. The keyless identity
//! (`certificate-identity-regexp`) and issuer (`certificate-oidc-issuer`) are
//! passed per call from resolved config, so any consumer verifies against their
//! own workflow identity; no secret is involved (keyless verification checks a
//! public transparency-log entry and certificate chain).

use std::path::Path;

use rskit_errors::AppResult;

/// Verifies a Sigstore bundle over a blob (e.g. Sigstore cosign `verify-blob
/// --bundle`).
pub trait SignatureVerifier: Send + Sync {
    /// Verify that `bundle` is a valid Sigstore bundle over `blob`, issued to a
    /// certificate whose identity matches `identity` and whose OIDC issuer is
    /// `issuer`.
    ///
    /// # Errors
    /// Fails closed when the bundle is invalid, the identity/issuer does not
    /// match, or the verifier binary is absent, so a tampered or foreign-signed
    /// manifest is never trusted.
    fn verify_blob(
        &self,
        blob: &Path,
        bundle: &Path,
        identity: &str,
        issuer: &str,
    ) -> AppResult<()>;
}
