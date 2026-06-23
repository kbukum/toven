//! Shared content-digest port double: [`FakeSourceDigest`].
//!
//! Cache-key tests substitute this deterministic digest so hashes are controlled
//! without touching the filesystem: each identity is derived from the queried
//! module ref or path rather than real file content.

use std::path::Path;

use rskit_errors::AppResult;
use toven_model::Module;
use toven_ports::SourceDigest;

/// A deterministic [`SourceDigest`]: a module/path identity is derived from its
/// own name, so tests control hashes without touching the filesystem.
#[derive(Debug, Default)]
pub struct FakeSourceDigest;

impl FakeSourceDigest {
    /// Construct the deterministic digest.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SourceDigest for FakeSourceDigest {
    fn module(&self, module: &Module) -> AppResult<String> {
        Ok(format!("module:{}", module.id))
    }

    fn path(&self, repo_relative: &Path) -> AppResult<String> {
        Ok(format!("path:{}", repo_relative.display()))
    }
}
