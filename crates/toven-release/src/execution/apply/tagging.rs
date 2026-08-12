use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_util::Template;
use toven_model::{Module, ModuleKey};
use toven_ports::{ReleaseVar, VcsWriter};

use crate::{ReleasePlan, ReleaseStats};

use super::staging::module_for;

/// Return the entry's already-planned tag name — a release uses the plan's
/// single planned value instead of re-deriving the tag from the scheme, so a
/// run creates, validates, names, and pushes precisely the tag the plan showed
/// — no second computation that could drift. A planned-version entry always
/// carries a planned tag (both are resolved together during planning); the
/// typed error guards that invariant without a panic.
pub(super) fn planned_tag_name(entry: &crate::ReleaseEntry) -> AppResult<&str> {
    entry.planned_tag.as_deref().ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!(
                "module '{}' has a planned version but no planned tag; the release plan is \
                 internally inconsistent",
                entry.module
            ),
        )
    })
}

/// Whether an entry's planned tag is created (and, on the maintainer path,
/// verified and pushed) under its resolved [`TagMode`](toven_ports::TagMode).
///
/// An unset `tag_mode` preserves the legacy layout: every planned tag is
/// created. An explicit mode gates by whether the entry is the umbrella module —
/// its tag is *the umbrella tag*, every other module's tag is a *per-module*
/// tag — so `PerModule` creates only per-module tags, `Umbrella` only the
/// umbrella tag, and `Both` creates both.
pub(super) fn entry_tag_selected(entry: &crate::ReleaseEntry) -> bool {
    entry.tag_mode.is_none_or(|mode| {
        if entry.umbrella {
            mode.creates_umbrella_tag()
        } else {
            mode.creates_per_module_tags()
        }
    })
}

/// Create every planned release tag against the release commit.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn tag_releases(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    writer: &dyn VcsWriter,
    commit: &toven_ports::Oid,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    let mut created = BTreeSet::new();
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            // The tag layout (per-module, umbrella, or both) decides which of a
            // train's tags are cut; a mode that skips this entry's tag creates
            // no tag for it.
            if !entry_tag_selected(entry) {
                continue;
            }
            let name = planned_tag_name(entry)?;
            // A single-version workspace collapses many modules onto one shared
            // tag (`tag_format = "v{version}"`): that is one release train,
            // created once, not one tag per module. The hosted-release phase
            // collapses the same modules onto one hosted Release identically.
            if !created.insert(name.to_string()) {
                continue;
            }
            let module = module_for(module_by_ref, &entry.module)?;
            let message = tag_message(entry, module, version)?;
            writer.create_tag(
                name,
                commit.as_str(),
                message.as_deref(),
                entry.signer.as_ref(),
            )?;
            stats.tagged_modules += 1;
        }
    }
    Ok(())
}

pub(super) fn render_template(
    template: &str,
    field: &str,
    module: &Module,
    version: &rskit_version::semver::Version,
    entry: &crate::ReleaseEntry,
) -> AppResult<String> {
    let parsed = Template::parse(template, ReleaseVar::ALL).map_err(|error| {
        AppError::invalid_input(field, format!("invalid release template: {error}"))
            .with_cause(error)
    })?;
    parsed
        .render_with(|placeholder| match placeholder {
            ReleaseVar::Version => Ok(version.to_string()),
            ReleaseVar::Ecosystem => Ok(module.id.ecosystem.to_string()),
            ReleaseVar::Module => Ok(module.id.name.clone()),
            ReleaseVar::Channel => Ok(entry.prerelease_channel.clone().unwrap_or_default()),
            _ => Err(AppError::new(
                ErrorCode::Internal,
                "unknown release template placeholder",
            )),
        })
        .map_err(|error| {
            AppError::invalid_input(field, format!("failed to render release template: {error}"))
                .with_cause(error)
        })
}

/// Render one module's optional annotation template.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn tag_message(
    entry: &crate::ReleaseEntry,
    module: &Module,
    version: &rskit_version::semver::Version,
) -> AppResult<Option<String>> {
    entry
        .tag_message
        .as_deref()
        .map(|template| render_template(template, "release.tag_message", module, version, entry))
        .transpose()
}

/// Refspecs pushed after tagging: the release commit's `branch` when it is
/// pushed (`Some`), plus every release tag.
///
/// The branch is pushed by its fully-qualified name (`refs/heads/<branch>`)
/// rather than `HEAD`: an ambiguous `HEAD` refspec depends on the remote's
/// `push.default` and silently fails to update the intended branch on a bare
/// remote, so the caller resolves the checked-out branch and pushes it
/// explicitly.
///
/// `None` selects the tags-only mode a protected branch requires, where the
/// release commit lands through a pull request rather than a direct branch
/// push: the branch ref is omitted, and because the branch name is never
/// needed the caller does not resolve it — a tags-only push also works from a
/// detached HEAD, the common CI checkout state.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn push_refspecs(plan: &ReleasePlan, branch: Option<&str>) -> AppResult<Vec<String>> {
    let mut refspecs = branch.map_or_else(Vec::new, |branch| vec![format!("refs/heads/{branch}")]);
    let mut seen = BTreeSet::new();
    for entry in &plan.entries {
        if entry.planned_version.is_some() {
            if !entry_tag_selected(entry) {
                continue;
            }
            let name = planned_tag_name(entry)?;
            // Modules sharing one collapsed tag push a single tag refspec.
            if seen.insert(name.to_string()) {
                refspecs.push(format!("refs/tags/{name}"));
            }
        }
    }
    Ok(refspecs)
}
