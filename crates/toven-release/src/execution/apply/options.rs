use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::Entrypoint;

use crate::PushPolicy;

/// Default rate-limit retry budget for the publish loop.
const DEFAULT_RETRY_BUDGET: usize = 5;

/// Runtime options for the release APPLY transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseApplyOptions {
    /// Suppress every config-permitted member push after tagging.
    pub no_push: bool,
    /// Publish the packaged artifacts to the registry after tagging. When
    /// false, the pipeline stops after commit/tag/push (the `release tag`
    /// surface).
    pub publish: bool,
    /// Maximum rate-limit retries per module in the publish loop.
    pub retry_budget: usize,
}

impl Default for ReleaseApplyOptions {
    fn default() -> Self {
        Self {
            no_push: true,
            publish: true,
            retry_budget: DEFAULT_RETRY_BUDGET,
        }
    }
}

/// Repository-scoped release settings reconciled from one member's plan entries.
///
/// A member release creates one commit and one push, so these settings cannot
/// vary among the modules it contains.
#[derive(Debug, Clone, Eq, PartialEq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct RepoReleaseSettings {
    push: PushPolicy,
    remote: String,
    branches: BTreeSet<String>,
    commit_message: Option<String>,
    entrypoint: Entrypoint,
}

impl RepoReleaseSettings {
    /// Whether this repository pushes after accounting for CLI suppression.
    #[must_use]
    pub(crate) const fn pushes(&self, options: &ReleaseApplyOptions) -> bool {
        self.push.permits_push() && !options.no_push
    }

    /// Whether the release commit's branch is pushed alongside the tags.
    #[must_use]
    pub(crate) const fn pushes_branch(&self) -> bool {
        self.push.pushes_branch()
    }

    /// Configured remote selected for the repository push.
    #[must_use]
    pub(crate) fn remote(&self) -> &str {
        &self.remote
    }

    /// Configured release-branch allow-list.
    #[must_use]
    pub(crate) const fn branches(&self) -> &BTreeSet<String> {
        &self.branches
    }

    /// Configured release-commit template, if any.
    #[must_use]
    pub(crate) fn commit_message(&self) -> Option<&str> {
        self.commit_message.as_deref()
    }

    /// Who cuts the release for this repository: Toven-owned (the default) or
    /// maintainer-owned (Toven runs against an existing human-created
    /// tag/Release).
    #[must_use]
    pub(crate) const fn entrypoint(&self) -> Entrypoint {
        self.entrypoint
    }
}

/// Reconcile settings that govern a single commit/push from member plan entries.
///
/// # Errors
/// Returns a typed configuration error when modules in the same repository
/// disagree on a repository-scoped setting.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn reconcile_repo_settings(
    entries: &[crate::ReleaseEntry],
) -> AppResult<RepoReleaseSettings> {
    let Some(first) = entries.first() else {
        return Err(AppError::new(
            ErrorCode::Internal,
            "cannot reconcile release settings for an empty repository plan",
        ));
    };
    let branches = first.branches.iter().cloned().collect::<BTreeSet<_>>();
    let settings = RepoReleaseSettings {
        push: first.push,
        remote: first.remote.clone(),
        branches,
        commit_message: first.commit_message.clone(),
        entrypoint: first.entrypoint,
    };
    for entry in entries.iter().skip(1) {
        if entry.push != settings.push {
            return repo_setting_conflict("push", first, entry);
        }
        if entry.remote != settings.remote {
            return repo_setting_conflict("remote", first, entry);
        }
        if entry.branches.iter().cloned().collect::<BTreeSet<_>>() != settings.branches {
            return repo_setting_conflict("branches", first, entry);
        }
        if entry.commit_message != settings.commit_message {
            return repo_setting_conflict("commit_message", first, entry);
        }
        if entry.entrypoint != settings.entrypoint {
            return repo_setting_conflict("entrypoint", first, entry);
        }
    }
    Ok(settings)
}

fn repo_setting_conflict(
    field: &str,
    first: &crate::ReleaseEntry,
    conflicting: &crate::ReleaseEntry,
) -> AppResult<RepoReleaseSettings> {
    Err(AppError::invalid_input(
        format!("release.{field}"),
        format!(
            "modules '{}' and '{}' resolve conflicting {field} settings in one repository",
            first.module, conflicting.module
        ),
    ))
}
