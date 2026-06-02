//! Discovery adapter contract.

use crate::core::{AppResult, DiscoverRequest, DiscoverResponse, Task, ToolchainProbe};

/// Discovers modules for one scope through a native or command-backed adapter.
pub trait DiscoveryAdapter: Send + Sync {
    /// Stable adapter identifier, for example `rust` or `command`.
    fn adapter_id(&self) -> &crate::core::AdapterId;

    /// Discover modules from the provided scope request.
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse>;

    /// Default tasks supplied by this adapter.
    fn default_tasks(&self) -> Vec<Task> {
        Vec::new()
    }

    /// Toolchain version probes included in cache identity for tasks planned by this adapter.
    fn toolchain_probes(&self) -> Vec<ToolchainProbe> {
        Vec::new()
    }
}
