//! Release-specific engine vocabulary and orchestration.

pub(crate) mod apply;
mod bump;
mod change;
mod changelog;
mod model;
pub(crate) mod plan;
pub(crate) mod publish;
mod rehearse;
mod run;
mod settings;
mod status;
mod strategy;
pub(crate) mod tag;
mod targets;

pub use apply::{ReleaseApplyOptions, release_apply};
pub use model::{
    ChangelogEntry, PublishDecision, RehearsalVerdict, ReleaseBaseline, ReleaseEntry,
    ReleaseModuleStatus, ReleasePlan, ReleaseRehearsal, ReleaseStats, ReleaseStatus,
    ReleaseStrategyName,
};
pub use plan::release_plan;
pub use rehearse::release_rehearse;
pub use run::release_run;
pub use settings::ResolvedReleaseSettings;
pub use status::release_status;
pub(crate) use targets::ReleaseTargets;
