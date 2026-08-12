//! The release APPLY transaction: clean-tree guardrail, manifest mutation,
//! packaging, a single release commit, per-module tagging, optional push, and
//! the bounded publish loop.
//!
//! The transaction has a hard commit-success boundary. Everything before a
//! successful commit (mutation + packaging + attempted commit) is undoable: any
//! failure restores the working tree and creates no commit or tag. Tags,
//! optional push, and the publish loop run after that boundary and are **not**
//! rolled back — a publish failure surfaces as a typed error and the operator
//! resumes, relying on registry idempotency.

mod guards;
mod options;
mod orchestration;
mod preflight;
mod staging;
mod tagging;
#[cfg(test)]
mod tests;

#[allow(clippy::redundant_pub_crate)]
pub(crate) use guards::{
    forward_recovery_error, guard_clean_tree, guard_release_branch, restore_or_precommit_error,
};
pub use options::ReleaseApplyOptions;
#[allow(clippy::redundant_pub_crate)]
pub(crate) use options::{RepoReleaseSettings, reconcile_repo_settings};
pub use orchestration::release_apply;
#[allow(clippy::redundant_pub_crate)]
pub(crate) use orchestration::verify_maintainer_tags;
#[allow(clippy::redundant_pub_crate)]
pub(crate) use preflight::{
    TagPreflight, preflight_tag_signers, preflight_tags, preflight_targets,
};
#[allow(clippy::redundant_pub_crate)]
pub(crate) use staging::{
    commit_message, module_for, package_publishable, prepare, publish_items, stage_and_commit,
    stage_only, target_for,
};
#[allow(clippy::redundant_pub_crate)]
pub(crate) use tagging::{push_refspecs, tag_releases};
