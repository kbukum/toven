//! Per-module release event projection.
//!
//! Maps [`ReleaseEntry`] decisions and committed member outcomes onto the
//! [`Event`] vocabulary. Two emissions per module: the *decision* before any
//! mutation, and the *commit* after the module's side effect has landed. The
//! library returns its typed aggregate as well; these emissions never print.

use rskit_version::semver::Version;
use toven_model::{Event, ModuleKey};

use crate::ReleaseEntry;

/// Project one module onto its per-module release *examining* event.
///
/// Advisory progress emitted **before** a module's slow decision I/O (baseline
/// resolution, change detection, registry lookup), so an operator sees
/// module-by-module motion during the seconds that are otherwise silent. It
/// never changes the run outcome and always precedes that module's settled
/// [`Event::ModuleReleaseResolved`] decision.
#[allow(clippy::redundant_pub_crate)]
#[must_use]
pub(crate) fn examining_event(module: &ModuleKey) -> Event {
    Event::ModuleReleaseExamining {
        module: module.to_string(),
    }
}

/// Project one plan entry onto its per-module release *decision* event.
///
/// A decision is a prediction from the resolved plan, identical to what
/// `release plan` and the bare-command preview report, so it is safe to emit
/// before any mutation.
#[allow(clippy::redundant_pub_crate)]
#[must_use]
pub(crate) fn resolved_event(entry: &ReleaseEntry) -> Event {
    Event::ModuleReleaseResolved {
        module: entry.module.to_string(),
        current_version: entry.current_version.to_string(),
        planned_version: entry.planned_version.as_ref().map(ToString::to_string),
        level: entry.level.as_str().to_string(),
        reason: entry.reason.as_str().to_string(),
        tag: entry.planned_tag.clone(),
        publication: Some(entry.publication.as_str().to_string()),
        up_to_date: entry.up_to_date,
    }
}

/// Project a module with no release work onto its settled decision event.
#[allow(clippy::redundant_pub_crate)]
#[must_use]
pub(crate) fn no_change_event(module: &ModuleKey, current_version: &Version) -> Event {
    Event::ModuleReleaseResolved {
        module: module.to_string(),
        current_version: current_version.to_string(),
        planned_version: None,
        level: "patch".to_string(),
        reason: "no-change".to_string(),
        tag: None,
        publication: None,
        up_to_date: false,
    }
}

/// Project one committed module onto its release *commit* event.
///
/// Constructed only after the module's side effect has landed (a `bump` stage,
/// or a `run` commit + tag), so a commit event never reports rolled-back work.
/// `new_version` is absent for a dependency-floor-only mutation, which stages a
/// rewritten manifest without cutting a new version of the module.
#[allow(clippy::redundant_pub_crate)]
#[must_use]
pub(crate) fn staged_event(
    module: &ModuleKey,
    new_version: Option<&Version>,
    manifests: Vec<String>,
    changelog: Option<String>,
    tag: Option<String>,
) -> Event {
    Event::ModuleReleaseStaged {
        module: module.to_string(),
        new_version: new_version.map(ToString::to_string),
        manifests,
        changelog,
        tag,
    }
}
