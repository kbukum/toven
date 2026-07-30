//! Non-secret credential *references* handed to a publish attempt.

/// The credential context for one publish attempt.
///
/// Carries the *name* of the environment variable that holds the registry
/// token — never the secret value. An adapter that publishes to a registry
/// reads that variable from its own process environment at publish time and
/// forwards the credential to its toolchain (for cargo, as
/// `CARGO_REGISTRY_TOKEN` on the child process environment, never on argv), so
/// the secret is read only at the toolchain boundary and never transits engine
/// memory, a log, or a command line. A tag-only target ignores it.
///
/// [`registry_token_env`](Self::registry_token_env) `None` means "use the
/// toolchain's ambient default credential" — the adapter injects nothing and
/// lets the toolchain resolve credentials as it normally would.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ReleaseCredentials {
    registry_token_env: Option<String>,
}

impl ReleaseCredentials {
    /// Build a credential context from the resolved `token_env` name (the
    /// environment variable that holds the registry token), or `None` for the
    /// toolchain's ambient default.
    #[must_use]
    pub const fn new(registry_token_env: Option<String>) -> Self {
        Self { registry_token_env }
    }

    /// The name of the environment variable holding the registry token, or
    /// `None` to use the toolchain's ambient default credential.
    #[must_use]
    pub fn registry_token_env(&self) -> Option<&str> {
        self.registry_token_env.as_deref()
    }
}
