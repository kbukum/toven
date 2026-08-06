//! Release PLAN tail: resolve the federated release plan and its read-only
//! projections — plan, combined run facade, readiness preflight, status, and
//! rehearsal.

#[allow(clippy::redundant_pub_crate)]
pub(crate) mod plan;
mod readiness;
mod rehearse;
mod run;
mod status;

pub use plan::release_plan;
pub use readiness::{ReadinessCheck, ReadinessReport, release_readiness};
pub use rehearse::release_rehearse;
pub use run::release_run;
pub use status::release_status;
