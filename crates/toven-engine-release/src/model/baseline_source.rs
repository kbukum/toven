//! [`BaselineSource`] — the release-owned policy vocabulary for *where* a
//! module's change-detection baseline comes from.
//!
//! A module's release baseline can be anchored on any of three interchangeable
//! sources, selectable per ecosystem and overridable in config: its own release
//! tag, an umbrella tag shared across a workspace, or the registry's max
//! published version. This enum names the choice; the resolver in
//! [`crate::versioning::baseline`] turns a choice into a concrete
//! [`ReleaseBaseline`](crate::ReleaseBaseline).
//!
//! It lives in the release model — not in `toven-ports`/`toven-model` — because
//! it is release *policy* and references [`TagScheme`], and it must not leak
//! upward into the reusable change foundation.

use toven_ports::TagScheme;

/// Where a module's release change-detection baseline is anchored.
///
/// Resolving a source yields a [`ReleaseBaseline`](crate::ReleaseBaseline) with
/// a version (the idempotency anchor) and, when a commit is available, a diff
/// ref (the file-diff anchor). The three variants mirror the two registry
/// models Toven supports — per-module tags *are* the registry (Go), while a
/// registry (crates.io) plus one umbrella tag describes a Rust workspace.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BaselineSource {
    /// The module's own latest release tag, matched by its own tag scheme — the
    /// per-module-tag model where a module carries independent release tags.
    OwnTag {
        /// The module's own tag scheme, matching tags like `rust/core@1.2.3`.
        scheme: TagScheme,
    },
    /// The latest tag matching a shared *umbrella* scheme (e.g. `v1.2.3`). The
    /// baseline version is the version that umbrella tag denotes and the diff
    /// ref is the umbrella tag's commit — the workspace-shared model where every
    /// module releases together under one repo tag.
    UmbrellaTag {
        /// The umbrella module's tag scheme, matching tags like `v1.2.3`.
        umbrella_scheme: TagScheme,
    },
    /// The registry's max published version anchors idempotency, while the diff
    /// ref is sourced from an inner tag anchor (`diff`). The effective baseline
    /// version is the **max** of the registry version and the `diff` anchor's
    /// version — the `max(registry max, version-at-tag)` composition a Rust
    /// workspace with crates.io history and one umbrella tag needs. A registry
    /// lookup failure downgrades to the `diff` anchor rather than aborting.
    Registry {
        /// The tag anchor supplying the diff ref (and a version to `max` with
        /// the registry version) — typically an [`OwnTag`](Self::OwnTag) or
        /// [`UmbrellaTag`](Self::UmbrellaTag).
        diff: Box<Self>,
    },
}

impl BaselineSource {
    /// Anchor on the module's own release tags.
    #[must_use]
    pub const fn own_tag(scheme: TagScheme) -> Self {
        Self::OwnTag { scheme }
    }

    /// Anchor on a shared umbrella tag scheme.
    #[must_use]
    pub const fn umbrella_tag(umbrella_scheme: TagScheme) -> Self {
        Self::UmbrellaTag { umbrella_scheme }
    }

    /// Anchor on the registry max published version, taking the diff ref from
    /// `diff` and the effective version from `max(registry, diff)`.
    #[must_use]
    pub fn registry(diff: Self) -> Self {
        Self::Registry {
            diff: Box::new(diff),
        }
    }
}
