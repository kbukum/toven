//! The pure version decision: the GATHER-fed inputs and the `plan_bumps`
//! function that turns them into a [`BumpPlan`].

mod inputs;
mod plan;

pub use inputs::{BumpConfig, CutIntent, ModuleVersionConfig, VersionInputs};
pub use plan::{BumpEntry, BumpPlan, BumpPlanner, BumpResolution, plan_bumps};
