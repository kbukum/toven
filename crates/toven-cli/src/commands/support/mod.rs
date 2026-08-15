//! Cross-verb CLI assembly shared by the execution verbs.
//!
//! The task-APPLY verbs ([`run`](super::run), [`watch`](super::watch)) all build
//! the same live-output APPLY host — a [`ProcessCommandRunner`] bound to the
//! resolved live view and the caller-owned process supervisor, plus a per-unit
//! [`UnitOutputChannel`] — so that bundle is assembled once here rather than
//! duplicated per verb. The read-only projection verbs ([`coverage`](super::coverage),
//! [`release`](super::release)) share the quiet reporter that surfaces only
//! warnings while stdout carries their projection.

pub(crate) mod live_apply;
pub(crate) mod reporter;

pub(crate) use live_apply::{LiveApplyBinding, build_live_apply_host};
pub(crate) use reporter::QuietReporter;
