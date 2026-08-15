//! Toven's versioning decision crate: the single **pure** bump-decision path.
//!
//! The version decision is one pure function, [`plan_bumps`], fed by
//! pre-gathered [`VersionInputs`]: GATHER (in `toven-release`) does every
//! git/ecosystem lookup a bump needs — declared versions, registry-published
//! versions, the already-resolved [`ReleaseBaseline`], change flags — and hands
//! them in as data. `plan_bumps` then resolves each module's independent bump,
//! cascades dependency floors, and pre-skips already-released versions, without
//! touching a `VcsReader`, an ecosystem adapter, or any I/O.
//!
//! Because baseline anchoring is an **input** ([`VersionInputs::baseline`],
//! produced by the pure [`resolve_baseline`]) rather than a step interleaved
//! with the decision, the two version-decision bugs that once hid there — an
//! umbrella anchor computed from the wrong version, and a maintainer echo that
//! skipped change-gating — become properties of a pure function, covered by
//! git-free tests.
//!
//! This crate owns the bump *policy* vocabulary ([`BumpPolicy`],
//! [`BumpReason`], [`BumpSource`]), the argv [`BumpOverrides`], the baseline
//! anchoring policy, and changelog generation. The semver *mechanism* it builds
//! on (version math, tag codec) lives in the pure [`toven_semver`] toolkit;
//! entry assembly, the impure GATHER wiring, and the `release bump` facade stay
//! in `toven-release`.

// Internal helpers live in private modules but are shared across sibling modules as
// `pub(crate)`. The `redundant_pub_crate` (nursery) lint would rather they be plain `pub`, but
// `unreachable_pub` then flags them as crate-internal — the two lints conflict for this shape.
// Allow the nursery lint crate-wide and keep the honest `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

pub mod changelog;

mod baseline;
mod conventional;
mod decision;
mod overrides;
mod policy;
mod strategy;

pub use baseline::{BaselineSource, ReleaseBaseline, resolve_baseline};
pub use changelog::ChangelogEntry;
pub use conventional::{
    CONVENTIONAL_COMMIT_TYPES, CommitLintViolation, ConventionalHeader,
    validate_conventional_subject,
};
pub use decision::{
    BumpConfig, BumpEntry, BumpPlan, BumpPlanner, BumpResolution, CutIntent, ModuleVersionConfig,
    VersionInputs, plan_bumps,
};
pub use overrides::BumpOverrides;
pub use policy::{BumpPolicy, BumpReason, BumpSource};
pub use strategy::resolve_bump_policy;
