//! [`SourceDigest`] — the injected content-digest port.

use std::path::Path;

use rskit_errors::AppResult;
use toven_model::Module;

/// A stable content identity for module sources and shared-input files.
///
/// Both methods return an opaque, stable identity string that changes iff the
/// hashed content changes. The string format is an adapter detail — callers
/// must treat it as opaque and compare it only for equality, never parse it or
/// assume a particular encoding (the filesystem adapter emits a hex hash; test
/// doubles may emit anything stable). A missing path hashes to a stable empty
/// identity rather than erroring, so an absent optional shared input does not
/// abort PLAN. Hashing is a filesystem side effect, so it is an injected port:
/// the planner stays pure and tests substitute a deterministic digest while the
/// concrete filesystem adapter lives in the engine.
pub trait SourceDigest {
    /// Content identity of a module's source tree (`module.root` subtree).
    ///
    /// # Errors
    /// Propagates a backing read failure.
    fn module(&self, module: &Module) -> AppResult<String>;

    /// Content identity of one workspace-relative shared input.
    ///
    /// The path may be a regular file or a directory; a directory is hashed as
    /// its whole subtree. A missing path hashes to the stable empty identity.
    ///
    /// # Errors
    /// Propagates a backing read failure.
    fn path(&self, repo_relative: &Path) -> AppResult<String>;
}
