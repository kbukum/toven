//! Language adapter contract.

use crate::core::{AppResult, DiscoverRequest, DiscoverResponse};

/// Discovers modules for one language or tool ecosystem.
pub trait LangAdapter: Send + Sync {
    /// Stable language identifier, for example `rust` or `python`.
    fn language(&self) -> &str;

    /// Discover modules from the provided workspace request.
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse>;
}
