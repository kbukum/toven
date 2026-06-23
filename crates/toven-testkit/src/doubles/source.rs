//! Shared content-digest port double: [`FakeSourceDigest`].
//!
//! Cache-key tests substitute this deterministic digest so hashes are controlled
//! without touching the filesystem: each identity is derived from the queried
//! module ref or path rather than real file content.
//!
//! To mirror the [`SourceDigest`] contract — where an absent module root or an
//! absent shared input hashes to a single stable empty identity rather than
//! erroring — entries marked absent via [`FakeSourceDigest::with_absent_path`] /
//! [`FakeSourceDigest::with_absent_module`] all return the same
//! [`EMPTY_IDENTITY`], so tests can model optional inputs that are not present.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use toven_model::{Module, ModuleRef};
use toven_ports::SourceDigest;

/// The stable identity every absent module/path hashes to, matching the
/// production `FsSourceDigest` empty-content digest (opaque: compare for
/// equality only, never parse).
pub const EMPTY_IDENTITY: &str = "empty";

/// A deterministic [`SourceDigest`] for tests.
///
/// A present module/path identity is derived from its own name, so tests control
/// hashes without touching the filesystem, while entries registered as absent
/// collapse to the shared [`EMPTY_IDENTITY`] — modeling the real planner's
/// "absent optional input" behavior.
#[derive(Debug, Default)]
pub struct FakeSourceDigest {
    absent_modules: BTreeSet<ModuleRef>,
    absent_paths: BTreeSet<PathBuf>,
}

impl FakeSourceDigest {
    /// Construct a digest where every module/path is treated as present.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Treat `module`'s source tree as absent: it hashes to [`EMPTY_IDENTITY`].
    #[must_use]
    pub fn with_absent_module(mut self, module: ModuleRef) -> Self {
        self.absent_modules.insert(module);
        self
    }

    /// Treat `path` as an absent shared input: it hashes to [`EMPTY_IDENTITY`].
    #[must_use]
    pub fn with_absent_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.absent_paths.insert(path.into());
        self
    }
}

impl SourceDigest for FakeSourceDigest {
    fn module(&self, module: &Module) -> AppResult<String> {
        if self.absent_modules.contains(&module.id) {
            return Ok(EMPTY_IDENTITY.to_string());
        }
        Ok(format!("module:{}", module.id))
    }

    fn path(&self, repo_relative: &Path) -> AppResult<String> {
        if self.absent_paths.contains(repo_relative) {
            return Ok(EMPTY_IDENTITY.to_string());
        }
        Ok(format!("path:{}", repo_relative.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};

    use super::{EMPTY_IDENTITY, FakeSourceDigest, SourceDigest};

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            RepoPath::new(name).unwrap(),
        )
    }

    #[test]
    fn present_inputs_get_distinct_derived_identities() {
        let digest = FakeSourceDigest::new();
        assert_eq!(digest.module(&module("a")).unwrap(), "module:rust:a");
        assert_eq!(
            digest.path(Path::new("Cargo.lock")).unwrap(),
            "path:Cargo.lock"
        );
        assert_ne!(
            digest.path(Path::new("a")).unwrap(),
            digest.path(Path::new("b")).unwrap()
        );
    }

    #[test]
    fn absent_inputs_collapse_to_the_shared_empty_identity() {
        let absent_module = module("gone").id;
        let digest = FakeSourceDigest::new()
            .with_absent_module(absent_module)
            .with_absent_path("optional/lock")
            .with_absent_path("optional/config");

        // Absent module and both absent paths hash to the SAME empty identity,
        // matching the production contract for absent optional inputs.
        assert_eq!(digest.module(&module("gone")).unwrap(), EMPTY_IDENTITY);
        assert_eq!(
            digest.path(Path::new("optional/lock")).unwrap(),
            EMPTY_IDENTITY
        );
        assert_eq!(
            digest.path(Path::new("optional/config")).unwrap(),
            EMPTY_IDENTITY
        );

        // A present sibling is unaffected.
        assert_eq!(digest.module(&module("here")).unwrap(), "module:rust:here");
    }
}
