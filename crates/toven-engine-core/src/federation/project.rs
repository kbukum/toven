//! The umbrella project's opened VCS provisioning for the CLI.
//!
//! Composes the umbrella into its members, resolves each member's change
//! baseline, and opens one deduped rskit-git reader/writer per distinct member
//! repo root. Both PLAN (change selection) and release (change detection plus
//! per-member commits) borrow this one opened set, so the single-repo project
//! is simply the N=1 degenerate member at the umbrella root.

use rskit_errors::AppResult;
use toven_model::AbsPath;
use toven_ports::Provider;

use crate::config::Document;
use crate::vcs::BaselineFlags;

use super::baseline::{OpenMemberVcsReaders, open_member_vcs_readers, resolve_baselines};
use super::spine;

/// Open the project's per-member VCS readers/writers.
///
/// Enumerates `[[members]]` (or the lone degenerate member), resolves every
/// member's effective change baseline against `flags`, and opens one rskit-git
/// adapter per distinct member repo root. The returned set is borrowed into the
/// PLAN reader view ([`OpenMemberVcsReaders::readers`]) and the release repo
/// ports ([`OpenMemberVcsReaders::release_repos`]).
///
/// # Errors
/// Propagates member composition (absent/escaping member root, missing member
/// `toven.toml`), baseline resolution, and repository open failures.
pub fn open_project_vcs(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    flags: &BaselineFlags,
) -> AppResult<OpenMemberVcsReaders> {
    let composed = spine::compose(project_root, document, providers)?;
    let baselines = resolve_baselines(&composed, flags);
    open_member_vcs_readers(
        project_root,
        &composed,
        &baselines,
        &document.toven.git.push_token_env,
    )
}
