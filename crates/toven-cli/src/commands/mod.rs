//! The command verbs: each reserved built-in (and the argv-first task escape
//! hatch) dispatches here from [`app`](crate::app).
//!
//! Every verb is a thin caller over the engine's PLAN/APPLY/release spine and the
//! reporter sinks: it builds the typed request, invokes the engine, and
//! lets the reporter render. Execution verbs live in [`run`]; the PLAN-cut
//! projections in [`introspect`]; the cache-maintenance verbs in [`cache`]; the
//! federation provisioning verbs plus the hidden `__serve` port-server entry in
//! [`driver`]; and the onboarding wizard in [`init`] are wired here as typed
//! command implementations.

pub(crate) mod cache;
pub(crate) mod completions;
pub(crate) mod driver;
pub(crate) mod init;
pub(crate) mod introspect;
pub(crate) mod release;
pub(crate) mod run;
pub(crate) mod selection;
pub(crate) mod tasks;
pub(crate) mod watch;
