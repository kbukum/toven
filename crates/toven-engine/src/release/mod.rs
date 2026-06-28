//! Release-specific engine vocabulary and orchestration.

use std::collections::BTreeMap;

use toven_model::{EcosystemId, MemberId};
use toven_ports::ReleaseTarget;

pub(crate) mod apply;
mod bump;
mod change;
mod changelog;
mod model;
pub(crate) mod plan;
pub(crate) mod publish;
mod run;
mod strategy;
pub(crate) mod tag;

pub use apply::{ReleaseApplyOptions, release_apply};
pub use model::{
    ChangelogEntry, ReleaseBaseline, ReleaseEntry, ReleasePlan, ReleaseStats, ReleaseStrategyName,
};
pub use plan::release_plan;
pub use run::release_run;

/// Release targets resolved per `(member, ecosystem)`.
///
/// Keying by member as well as ecosystem keeps each federation member's
/// publishability authoritative: two members exposing the same ecosystem (e.g.
/// `rust`) can disagree on `publish`, so a publishable member must never cause
/// a `publish = false` member's modules in that ecosystem to be released. The
/// single-repo case is one entry under the `None` member.
pub(crate) type ReleaseTargets = BTreeMap<(Option<MemberId>, EcosystemId), Box<dyn ReleaseTarget>>;
