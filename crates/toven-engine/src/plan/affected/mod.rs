//! Affected: resolve a plan request's selection to the active module set.
//!
//! The active set is the input to scheduling. It is derived one of two ways:
//! - [`select`] resolves an explicit user selection (`--module`/`--workspace`
//!   selectors, optionally expanded through the dependency/dependents
//!   closures);
//! - [`changed`] maps changed paths to owning modules via longest-prefix roots
//!   and adapter blast-radius globs, then fails closed to the full set on any
//!   unclassifiable path.
//!
//! [`entry`] holds the dispatcher that picks the strategy for a request.

mod changed;
mod entry;
mod select;

#[cfg(test)]
mod tests;

#[allow(clippy::redundant_pub_crate)]
pub(crate) use changed::{changed_for_members, changed_records_for_module, changed_seeds};
#[allow(clippy::redundant_pub_crate)]
pub(crate) use entry::{active_modules, restrict_to_task_defining};
