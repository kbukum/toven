//! The command verbs: each reserved built-in (and the argv-first task escape
//! hatch) dispatches here from [`app`](crate::app).
//!
//! Every verb is a thin caller over the engine's PLAN/APPLY/release spine and the
//! step-7 reporter sinks: it builds the typed request, invokes the engine, and
//! lets the reporter render. Execution verbs live in [`run`]; the PLAN-cut
//! projections in [`introspect`]; the cache-maintenance verbs in [`cache`]; the
//! federation provisioning verbs plus the hidden `__serve` port-server entry in
//! [`driver`]; and the verbs whose behavior lands in later steps ([`generate`])
//! are wired here as typed "not yet implemented" stubs so the surface is complete.

pub(crate) mod cache;
pub(crate) mod driver;
pub(crate) mod generate;
pub(crate) mod introspect;
pub(crate) mod run;
