//! The typed value inputs and configuration for the pure version decision.
//!
//! GATHER pre-fetches every git/ecosystem fact a module's bump needs into a
//! [`VersionInputs`] value — its declared version, its registry-published
//! versions, its already-resolved [`ReleaseBaseline`], whether it changed, and
//! the decision-relevant slice of its resolved release config — so
//! [`plan_bumps`](super::plan_bumps) never touches an adapter, a `VcsReader`, or
//! any I/O. [`BumpConfig`] carries the run-wide pure data (the dependency graph
//! and edges, the checked-out branches, the selected policy, the argv
//! overrides, and the cut intent).

use std::collections::BTreeMap;

use rskit_version::semver::Version;
use toven_model::{Entrypoint, Graph, MemberId, ModuleKey};
use toven_ports::{BumpLevel, DependentVersion, PrereleaseConfig, PublicationPolicy};

use crate::baseline::ReleaseBaseline;
use crate::overrides::BumpOverrides;
use crate::policy::BumpPolicy;

/// What a bump plan is for: a preview, a `bump` mutation, or a verify cut.
///
/// A read-only projection (`release plan` and the other previews), the
/// standalone `release bump` mutation, or the cut a verify-and-publish run
/// (`release tag`/`publish`) will apply. Two axes differ across the three
/// intents:
/// - **manifest floor.** A projection reports a manifest version that is not
///   ahead of its released baseline as nothing-to-release; a `bump` likewise
///   drops it (there is nothing to advance); only a verify run fails closed so
///   it never re-cuts an already-released version.
/// - **maintainer-owned reach.** `plan`/`publish` force-include every
///   maintainer-owned module to verify its out-of-band tag; `bump` must not —
///   a maintainer-owned module whose manifest is not ahead of its baseline has
///   nothing to bump, so it stays out of the bump set.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum CutIntent {
    /// A read-only projection: a not-ahead manifest version is a no-op, not an
    /// error.
    Preview,
    /// The standalone `release bump` mutation: a not-ahead manifest version is a
    /// no-op (nothing to advance), and maintainer-owned modules are not
    /// force-included.
    Bump,
    /// A verify-and-publish cut (`release tag`/`publish`): a not-ahead manifest
    /// version fails closed, and maintainer-owned modules are force-included to
    /// verify their tags.
    Verify,
}

impl CutIntent {
    /// Whether a manifest version that is not ahead of its released baseline
    /// fails the run closed (`Verify`) rather than resolving to
    /// nothing-to-release (`Preview`/`Bump`).
    pub(crate) const fn not_ahead_is_fatal(self) -> bool {
        matches!(self, Self::Verify)
    }

    /// Whether change detection force-includes every maintainer-owned module to
    /// verify its out-of-band tag. Only the verify-and-publish path does; a
    /// `bump` reaches a maintainer-owned module only when it genuinely changed.
    #[must_use]
    pub const fn forces_maintainer_owned(self) -> bool {
        matches!(self, Self::Verify)
    }

    /// Whether a maintainer-owned module **echoes** its already-declared version
    /// (the verify-and-publish path) instead of computing a bump.
    ///
    /// Only `release tag`/`publish` (`Verify`) verify a version a maintainer
    /// already merged and tagged out of band. `bump` and `plan` still compute
    /// the increment (change-gated, cascaded) that the maintainer then reviews
    /// and merges — so a maintainer-owned workspace is not frozen to its
    /// declared versions at bump time, it just owns the commit/tag/push.
    pub(crate) const fn verifies_maintainer_version(self) -> bool {
        matches!(self, Self::Verify)
    }
}

/// The decision-relevant slice of a module's resolved release config.
///
/// A pure projection of `ResolvedReleaseSettings` (owned by `toven-release`)
/// carrying only the fields the bump decision consults, so the decision crate
/// never depends on the full release settings type. GATHER copies these across
/// when it assembles [`VersionInputs`].
#[derive(Debug, Clone)]
pub struct ModuleVersionConfig {
    /// Configured own-version bump level (`auto` unless pinned).
    pub level: BumpLevel,
    /// How a dependency-floor cascade advances a dependent's own version.
    pub dependent_version: DependentVersion,
    /// Prerelease channels and the branch→channel mapping.
    pub prerelease: PrereleaseConfig,
    /// Whether the module publishes to a registry or is tag-only (and whether it
    /// releases at all).
    pub publication: PublicationPolicy,
    /// Whether registry lookups are skipped, anchoring idempotency on the
    /// release tag only.
    pub offline: bool,
    /// Who cuts the release: Toven or a maintainer (out-of-band tag).
    pub entrypoint: Entrypoint,
}

/// Everything the pure decision needs to know about one module, pre-gathered.
///
/// All git/ecosystem I/O is done **before** the decision and captured here: the
/// adapter-declared `current_version`, the registry's `published_versions`
/// (empty offline or on a lookup failure), the already-resolved `baseline`
/// (via [`resolve_baseline`](crate::resolve_baseline)), whether the module
/// `changed` since that baseline, whether its changelog classification is
/// `breaking`, and the decision-relevant `config`.
#[derive(Debug, Clone)]
pub struct VersionInputs {
    /// The module this input describes.
    pub module: ModuleKey,
    /// The version the module's manifest currently declares.
    pub current_version: Version,
    /// The registry's published versions for the module (empty offline or when
    /// a lookup failed — treated as "publish needed").
    pub published_versions: Vec<Version>,
    /// The module's resolved release baseline (its idempotency + diff anchor),
    /// pre-computed in GATHER so the decision reads it as data.
    pub baseline: ReleaseBaseline,
    /// Whether the module changed since its baseline (a changed seed).
    pub changed: bool,
    /// Whether the module's changelog classification marks a breaking change.
    pub breaking: bool,
    /// The decision-relevant slice of the module's resolved release config.
    pub config: ModuleVersionConfig,
}

/// The run-wide pure configuration for a [`plan_bumps`](super::plan_bumps) call.
pub struct BumpConfig<'a> {
    /// The federated dependency graph (used for the release closure, ranks, and
    /// cascade floors — cascade edges are read from [`Graph::edges`]).
    pub graph: &'a Graph,
    /// Each member's checked-out branch, consulted only to resolve a configured
    /// branch→prerelease-channel mapping.
    pub branches: &'a BTreeMap<Option<MemberId>, String>,
    /// The selected engine-owned bump policy.
    pub policy: BumpPolicy,
    /// The validated per-run bump argv overrides.
    pub overrides: &'a BumpOverrides,
    /// Whether this cut is a read-only projection, a `bump`, or a
    /// verify-and-publish run.
    pub intent: CutIntent,
}
