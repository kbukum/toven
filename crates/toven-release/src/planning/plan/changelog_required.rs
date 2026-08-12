use std::collections::BTreeMap;
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::read_string_bounded;
use toven_model::ModuleKey;
use toven_version::changelog;

use crate::ResolvedReleaseSettings;
use crate::versioning::change;

/// Fail closed when a directly changed release unit lacks its required,
/// file-backed changelog evidence.
///
/// The configured changelog is read from its project-relative path and must
/// carry a documented `## [Unreleased]` section (see
/// [`changelog::unreleased_documented`]). A missing, unreadable, or
/// undocumented changelog is a typed configuration failure surfaced before any
/// mutation. Modules selected only through a dependency cascade are not directly
/// changed and are exempt — their release reason is the cascade explanation
/// carried by their [`ChangelogEntry`](crate::ChangelogEntry).
pub(super) fn validate_required_changelogs(
    project_root: &Path,
    changes: &change::ReleaseChanges,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<()> {
    /// Upper bound on a changelog read; a document larger than this is treated
    /// as malformed rather than loaded unbounded.
    const MAX_CHANGELOG_BYTES: u64 = 4 * 1024 * 1024;

    let mut documented: BTreeMap<String, bool> = BTreeMap::new();
    for module in &changes.changed {
        let Some(resolved) = settings.get(module) else {
            continue;
        };
        if !resolved.changelog.required {
            continue;
        }
        let relative = resolved.changelog.path.as_deref().unwrap_or("CHANGELOG.md");
        if let Some(has_entry) = documented.get(relative) {
            if !*has_entry {
                return Err(undocumented_changelog_error(module, relative));
            }
            continue;
        }
        let absolute = safe_join(project_root, relative).map_err(|error| {
            AppError::invalid_input(
                "release.changelog.path",
                format!("changelog path '{relative}' is not a safe project-relative path"),
            )
            .with_cause(error)
        })?;
        let text = read_string_bounded(&absolute, MAX_CHANGELOG_BYTES).map_err(|error| {
            AppError::invalid_input(
                "release.changelog.required",
                format!(
                    "required changelog '{relative}' for changed module '{module}' could not be \
                     read; create it and document the change before releasing"
                ),
            )
            .with_cause(error)
        })?;
        let has_entry = changelog::unreleased_documented(&text);
        documented.insert(relative.to_string(), has_entry);
        if !has_entry {
            return Err(undocumented_changelog_error(module, relative));
        }
    }
    Ok(())
}

/// The typed failure for a changed module whose required changelog has no
/// documented `[Unreleased]` entry.
fn undocumented_changelog_error(module: &ModuleKey, relative: &str) -> AppError {
    AppError::invalid_input(
        "release.changelog.required",
        format!(
            "changed module '{module}' requires a documented '[Unreleased]' entry in \
             '{relative}', but none was found; record the change before releasing"
        ),
    )
}
