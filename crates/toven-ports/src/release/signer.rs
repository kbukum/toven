//! Signer port: sign a release blob, producing a detached signature and the
//! signing certificate.
//!
//! Signing *policy* — which blob is signed, the keyless-vs-keyed identity
//! selection, and the output asset names — is release-engine domain. This port
//! is the thin reusable sliver: run a signer over a blob. The identity
//! selection is passed per call (`signer`); the adapter never returns, logs, or
//! embeds a secret — keyless Sigstore mints an ephemeral certificate from the
//! ambient OIDC identity, and a named key ref selects a key without carrying its
//! material.

use std::path::Path;

use rskit_errors::AppResult;

/// Signs a release blob with a keyless or keyed signer (e.g. Sigstore cosign).
pub trait Signer: Send + Sync {
    /// Sign `blob`, writing the detached signature to `signature` and the
    /// signing certificate to `certificate`. `signer` selects the identity/key
    /// (never a secret): `None` is the keyless default; `Some(ref)` names a key.
    ///
    /// # Errors
    /// Fails closed when the signer binary is absent or signing fails, so the
    /// release never proceeds with an unsigned artifact.
    fn sign_blob(
        &self,
        blob: &Path,
        signature: &Path,
        certificate: &Path,
        signer: Option<&str>,
    ) -> AppResult<()>;
}
