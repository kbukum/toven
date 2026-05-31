//! Discovery adapter contract.

use crate::core::{AppResult, DiscoverRequest, DiscoverResponse};

/// Discovers modules for one scope through a native or command-backed adapter.
pub trait DiscoveryAdapter: Send + Sync {
    /// Stable adapter identifier, for example `rust` or `command`.
    fn adapter_id(&self) -> &crate::core::AdapterId;

    /// Discover modules from the provided scope request.
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse>;
}
