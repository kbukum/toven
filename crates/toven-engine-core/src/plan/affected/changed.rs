//! Member-scoped change collection: compose each member repo's committed and
//! working-tree changes into one record set.
//!
//! Ownership resolution (which module a changed path belongs to) is the
//! separate [`ownership`](crate::plan::ownership) concern; this module only
//! collects the raw [`ChangeRecord`]s per member repo, applying the request's
//! baseline fallback and umbrella prefix rebasing.

use rskit_errors::AppResult;
use toven_ports::{BaselineSpec, ChangeRecord};

use crate::federation::baseline::{MemberVcsReader, MemberVcsReaders};

/// Collect the changed records across every member repo, applying `fallback`
/// as the per-member baseline when a member declares no baseline of its own.
///
/// # Errors
/// Propagates any member VCS reader's change-detection failure.
pub fn changed_for_members(
    readers: &MemberVcsReaders<'_>,
    fallback: Option<&BaselineSpec>,
) -> AppResult<Vec<ChangeRecord>> {
    let mut changed = Vec::new();
    for reader in readers.entries() {
        changed.extend(changed_for_member(reader, fallback)?);
    }
    Ok(changed)
}

/// Map one member's changed paths since its baseline.
///
/// The member reader's own resolved baseline takes precedence; when it has none
/// the request's
/// [`Selection::Changed`](crate::plan::request::Selection::Changed) spec is the
/// fallback, so the variant's payload stays meaningful and the single-repo /
/// unconfigured-member case still resolves a baseline instead of failing.
fn changed_for_member(
    reader: &MemberVcsReader<'_>,
    fallback: Option<&BaselineSpec>,
) -> AppResult<Vec<ChangeRecord>> {
    let baseline = reader.baseline().or(fallback).ok_or_else(|| {
        rskit_errors::AppError::invalid_input(
            "base_ref",
            format!(
                "no baseline reference for member '{}': pass --base <ref> or set [[members]].base_ref / [project].base_ref",
                reader.member().map_or("<root>", toven_model::MemberId::as_str)
            ),
        )
    })?;
    let mut changed = reader.reader().changed_since(baseline)?;
    changed.extend(reader.reader().worktree_status()?);
    Ok(reader.umbrella_records(&changed))
}
