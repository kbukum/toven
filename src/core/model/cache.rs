//! Cache policy model.

/// Normalized cache settings.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CacheSettings {
    /// Cache storage location.
    pub location: CacheLocation,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            location: CacheLocation::User,
        }
    }
}

/// Cache storage location.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CacheLocation {
    /// Platform user cache directory.
    User,
    /// Workspace-local `.toven/cache` directory.
    Workspace,
}
