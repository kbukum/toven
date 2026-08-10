//! [`ReleaseDefaultsSource`] — an adapter's per-ecosystem default release model.
//!
//! The tag layout ([`TagMode`]) and change-detection anchor
//! ([`BaselineSourceConfig`]) a release train uses are correct out of the box
//! per ecosystem, yet both remain overridable in config. A registry-backed
//! ecosystem (crates.io) anchors on the registry plus one umbrella tag and cuts
//! per-module *and* umbrella tags for traceability, while an ecosystem whose
//! per-module tags *are* the registry (Go modules) anchors on each module's own
//! tag and cuts only per-module tags. Each adapter states its default here; the
//! release engine folds it into the resolved settings under the documented
//! precedence (`[modules.<name>.release]` > `[ecosystems.<id>].release` >
//! adapter default), so an explicit config value always wins.

use crate::config::{BaselineSourceConfig, TagMode};

/// An ecosystem adapter's default tag layout and baseline anchor.
///
/// The two knobs are orthogonal — the [`tag_mode`](Self::tag_mode) governs
/// *what tags are created* and the [`baseline`](Self::baseline) governs *what
/// change-gating diffs against* — but an ecosystem's correct defaults for the
/// two go together, so an adapter states them as one value. Both are
/// config-surface selectors ([`BaselineSourceConfig`] / [`TagMode`]), not
/// resolved release-model values: the engine resolves the selector against the
/// member's umbrella presence, degrading an umbrella-anchored default to its
/// per-module counterpart when the member declares no umbrella module.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub struct ReleaseDefaults {
    /// Where change detection anchors a module's baseline by default.
    pub baseline: BaselineSourceConfig,
    /// Which tags the train creates by default.
    pub tag_mode: TagMode,
}

impl ReleaseDefaults {
    /// Construct a default baseline/tag-mode pair.
    #[must_use]
    pub const fn new(baseline: BaselineSourceConfig, tag_mode: TagMode) -> Self {
        Self { baseline, tag_mode }
    }
}

/// State an ecosystem adapter's default release model.
///
/// The engine consults this only where the resolved config leaves the
/// [`baseline`](ReleaseDefaults::baseline) or [`tag_mode`](ReleaseDefaults::tag_mode)
/// unset; an explicit `[…].release` value takes precedence. Object-safe so the
/// engine can hold it behind [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait ReleaseDefaultsSource {
    /// This ecosystem's default tag layout and baseline anchor.
    fn release_defaults(&self) -> ReleaseDefaults;
}
