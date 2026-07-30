//! Downloader port: fetch named release assets from the hosted release into a
//! local directory.
//!
//! Download *policy* — which tag, which asset names, and the fail-closed order
//! in which they are verified — is release-engine domain. This port is the thin
//! reusable sliver: fetch these named assets for this tag into this directory.
//! Authentication stays ambient (the CI runner's `gh`/forge credentials); the
//! adapter never embeds or logs a token.

use std::path::Path;

use rskit_errors::AppResult;

/// Fetches named assets from a hosted release (e.g. `gh release download`).
pub trait AssetDownloader: Send + Sync {
    /// Download each asset in `assets` for release `tag` into `dest`.
    ///
    /// # Errors
    /// Fails closed when the release or a requested asset cannot be fetched, so
    /// verification never proceeds against a missing or partial download.
    fn download(&self, tag: &str, assets: &[&str], dest: &Path) -> AppResult<()>;
}
