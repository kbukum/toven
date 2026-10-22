//! The `release` lifecycle verb: `plan`, `status`, `bump`, `tag`, `publish`.
//!
//! `plan` and `status` are read-only projections over the engine release spine
//! — they render typed data on stdout and never mutate a manifest, tag, or
//! registry. `bump` runs only the version + changelog mutation phase, then
//! stages the mutation for a pull request without committing, tagging, pushing,
//! or publishing. `tag` and `publish` drive the mutating release pipeline
//! ([`release_run`](toven_release::release_run)): `tag` stops after the
//! release commit/tag/push, `publish` continues to the registry. `publish`
//! under `--dry-run` instead runs a no-mutation rehearsal
//! ([`release_rehearse`](toven_release::release_rehearse)) that reports the
//! resolved publish order and per-module
//! would-publish/already-published verdicts. Libraries return typed data; this
//! CLI layer is the only one that prints, following the introspection stream
//! convention (projection on stdout, warnings/summaries on stderr).

mod artifacts;
mod dispatch;
mod inspect;
mod lifecycle;
mod projectors;
mod summary;

#[cfg(test)]
mod tests;

pub(crate) use dispatch::execute;
