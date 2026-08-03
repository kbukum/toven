//! Signing material for signed release tags.

/// Object-signing backend for a signed tag.
///
/// Mirrors git's `gpg.format` backends so a release can pin how its tags are
/// signed rather than depending on ambient git configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignFormat {
    /// `OpenPGP` signatures via `gpg` (git's default backend).
    OpenPgp,
    /// SSH signatures via `ssh-keygen -Y sign` (git >= 2.34).
    Ssh,
    /// X.509 / S-MIME signatures via `gpgsm`; also the Sigstore `gitsign` path.
    X509,
}

/// Signing material for a signed, annotated tag.
///
/// A `None` field inherits the repository's git configuration (`gpg.format`,
/// `user.signingkey`); an explicit value pins that aspect for deterministic,
/// reproducible signing. Presence of a `TagSigner` at a `create_tag` call
/// requests a signed tag; absence requests an unsigned one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagSigner {
    /// Signing backend to select (`gpg.format`). `None` inherits git config.
    pub format: Option<SignFormat>,
    /// Signing key to use (`user.signingkey`). `None` inherits git config.
    pub key: Option<String>,
}
