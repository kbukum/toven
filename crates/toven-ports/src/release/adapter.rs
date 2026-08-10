//! [`ReleaseAdapter`] — the composed native release seam over the per-phase
//! contracts.

use super::{
    ManifestMutator, Packager, Publisher, ReleaseDefaultsSource, SbomProducer, TagGrammar,
    VersionSource,
};

/// The composed ecosystem release seam: every per-phase contract a native
/// adapter satisfies.
///
/// This is a **composition marker**, not a phase contract of its own — it names
/// no methods, it only bundles the per-phase traits
/// ([`VersionSource`], [`TagGrammar`], [`Packager`], [`ManifestMutator`],
/// [`Publisher`], [`SbomProducer`]) plus the adapter's default release model
/// ([`ReleaseDefaultsSource`]) so
/// [`ConfiguredAdapter::release_target`](crate::provider::ConfiguredAdapter::release_target)
/// can hand back one native trait object the engine resolves per phase. Each
/// phase can independently be backed `Native` (this adapter) or `Delegated` (an
/// external tool via [`DelegatedPhase`](super::DelegatedPhase)); the composite
/// never bundles behavior that would make delegation all-or-nothing.
///
/// The blanket implementation makes any type that satisfies all per-phase
/// contracts plus [`ReleaseDefaultsSource`] a `ReleaseAdapter`, so an ecosystem
/// adapter (e.g. the cargo or Go target) implements the phase traits it needs,
/// states its default release model, and gains the composite for free.
pub trait ReleaseAdapter:
    VersionSource
    + TagGrammar
    + Packager
    + ManifestMutator
    + Publisher
    + SbomProducer
    + ReleaseDefaultsSource
{
}

impl<T> ReleaseAdapter for T where
    T: VersionSource
        + TagGrammar
        + Packager
        + ManifestMutator
        + Publisher
        + SbomProducer
        + ReleaseDefaultsSource
{
}
