//! The hosted-release port — cutting a forge Release (GitHub/GitLab) for a
//! resolved tag, after the tag and registry publish phases.
//!
//! The port is forge-facing, not ecosystem-facing: a single adapter cuts the
//! Release for every target's tag over the one topological order. The engine
//! owns tag resolution, note sourcing, asset selection, ordering, and
//! reporting; the adapter owns only an idempotent create-or-update against one
//! forge.

use std::path::{Path, PathBuf};

use rskit_errors::AppResult;

/// Forge identifiers the hosted-release phase can cut a Release on.
///
/// This is the single source of truth for recognized forges: config validation
/// rejects any other `forge` value up front — before a run tags or publishes —
/// and each engine adapter maps exactly one of these identifiers.
pub const SUPPORTED_FORGES: &[&str] = &["github"];

/// Whether `forge` names a hosted-release forge the system supports.
#[must_use]
pub fn is_supported_forge(forge: &str) -> bool {
    SUPPORTED_FORGES.contains(&forge)
}

/// One artifact uploaded to a hosted release.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseAsset {
    /// Location of the artifact on disk.
    pub path: PathBuf,
    /// Optional display label; `None` uses the file name.
    pub label: Option<String>,
}

impl ReleaseAsset {
    /// Construct an asset at `path` with no display label.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            label: None,
        }
    }

    /// Attach a display label to the asset.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// A hosted forge Release to create or update for one resolved tag.
///
/// Fully resolved by the engine: `tag` is the already-formatted release tag,
/// `notes` the resolved note body (changelog or override), and `assets` the
/// bounded set of files to upload. The adapter never re-derives any of these.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HostedRelease {
    /// The release tag the hosted Release is cut against.
    pub tag: String,
    /// Human-readable release title.
    pub title: String,
    /// Resolved release note body.
    pub notes: String,
    /// Whether the Release is a draft (unpublished on the forge).
    pub draft: bool,
    /// Whether the Release is marked as a prerelease.
    pub prerelease: bool,
    /// Bounded set of artifacts uploaded to the Release.
    pub assets: Vec<ReleaseAsset>,
}

impl HostedRelease {
    /// Construct a hosted release for `tag` with the given `title` and `notes`.
    #[must_use]
    pub fn new(tag: impl Into<String>, title: impl Into<String>, notes: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            title: title.into(),
            notes: notes.into(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        }
    }

    /// Mark the release as a draft.
    #[must_use]
    pub const fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }

    /// Mark the release as a prerelease.
    #[must_use]
    pub const fn with_prerelease(mut self, prerelease: bool) -> Self {
        self.prerelease = prerelease;
        self
    }

    /// Set the uploaded artifact set.
    #[must_use]
    pub fn with_assets(mut self, assets: Vec<ReleaseAsset>) -> Self {
        self.assets = assets;
        self
    }
}

/// The outcome of ensuring a hosted Release exists on the forge.
///
/// Hosted publication is immutable: a Release is either newly created or an
/// identical one already existed and was verified. A conflicting existing
/// Release is never edited in place — the adapter fails instead, so this enum
/// has no "updated" outcome.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum HostReleaseOutcome {
    /// No Release existed for the tag, so a new one was created.
    Created,
    /// A Release already existed for the tag and was verified byte-identical to
    /// the intended one (an idempotent re-run), so nothing was mutated.
    AlreadyComplete,
}

impl HostReleaseOutcome {
    /// Canonical wire/report name for the outcome.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::AlreadyComplete => "already-complete",
        }
    }
}

/// The hosted-release forge port: cut a Release for a resolved tag immutably.
///
/// Object-safe so an engine host registry can hand back a `Box<dyn
/// ReleaseHost>` keyed by the configured forge. Implementations invoke their
/// forge CLI argv-only (never a shell string) and read any token from the
/// ambient environment only — never logging it.
pub trait ReleaseHost {
    /// Create the hosted Release for `release.tag`, or verify an identical one
    /// already exists (immutable create-or-verify).
    ///
    /// If no Release exists for the tag, one is created. If a Release already
    /// exists, it is compared field-by-field (title, notes, draft/prerelease
    /// flags, and every asset's name and size) against the intended release: an
    /// exact match reports [`HostReleaseOutcome::AlreadyComplete`]; any
    /// divergence is a typed conflict error with forward-fix guidance. An
    /// existing Release is never edited, and assets are never clobbered. The
    /// working directory `root` locates the forge repository the Release
    /// belongs to.
    ///
    /// # Errors
    /// Propagates a forge CLI spawn/IO failure or a non-zero CLI exit, and
    /// returns a typed conflict error when an existing Release diverges from the
    /// intended one.
    fn ensure_release(&self, root: &Path, release: &HostedRelease)
    -> AppResult<HostReleaseOutcome>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{HostReleaseOutcome, HostedRelease, ReleaseAsset};

    #[test]
    fn asset_defaults_to_no_label_and_takes_one() {
        let bare = ReleaseAsset::new("dist/app.tgz");
        assert_eq!(bare.path, PathBuf::from("dist/app.tgz"));
        assert_eq!(bare.label, None);

        let labeled = ReleaseAsset::new("dist/app.tgz").with_label("App");
        assert_eq!(labeled.label.as_deref(), Some("App"));
    }

    #[test]
    fn hosted_release_builder_sets_flags_and_assets() {
        let release = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "notes")
            .with_draft(true)
            .with_prerelease(true)
            .with_assets(vec![ReleaseAsset::new("dist/core.cdx.json")]);

        assert_eq!(release.tag, "rust/core@1.2.3");
        assert_eq!(release.title, "core 1.2.3");
        assert_eq!(release.notes, "notes");
        assert!(release.draft);
        assert!(release.prerelease);
        assert_eq!(release.assets.len(), 1);
    }

    #[test]
    fn outcome_names_are_stable() {
        assert_eq!(HostReleaseOutcome::Created.as_str(), "created");
        assert_eq!(
            HostReleaseOutcome::AlreadyComplete.as_str(),
            "already-complete"
        );
    }
}
