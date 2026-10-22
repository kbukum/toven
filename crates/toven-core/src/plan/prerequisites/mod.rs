//! Task prerequisites: a task's `depends_on` pulls other tasks into the same
//! plan and orders them first.
//!
//! - [`validate`] — reject unknown, persistent, or cyclic prerequisites and
//!   unsafe `outputs` at Configure time, before any module is discovered;
//!   [`validate_overrides`] applies the same rules to group `depends_on`,
//!   `persistent` and `outputs` overrides once the Graph phase resolves group
//!   membership.
//! - [`expand`] — after Affected, walk the requested (task, module) pairs
//!   through `depends_on` and add the prerequisite pairs as extra modules on
//!   per-task requests.
//! - [`link`] — after Schedule, add the cross-task unit edges and re-level the
//!   merged plan into dependency-respecting waves.

mod expand;
mod link;
mod validate;

#[cfg(test)]
mod tests;

pub(in crate::plan) use expand::{Needs, Scope, expand};
pub(in crate::plan) use link::link;
pub(in crate::plan) use validate::{validate, validate_overrides};
