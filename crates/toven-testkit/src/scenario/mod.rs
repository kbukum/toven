//! The declarative scenario format for Toven's golden end-to-end tests.
//!
//! A scenario is one `scenario.yaml` describing a *session*: a fixture repo,
//! optional scripted git history, toolchain gates, deterministic env, and an
//! ordered list of `toven` invocations with per-stream golden expectations and
//! declarative side-effects. This module owns the schema ([`Scenario`] and
//! friends), the mapping onto rskit matcher tiers ([`StreamExpectation::to_match`]),
//! the typed loader ([`Scenario::load`]), and the engine that executes a
//! session ([`run_scenario`]) — materialize, git-script, run each step
//! in-repo, verify streams and effects, and report per-step outcomes.

mod discover;
mod effects;
mod load;
mod matcher_kind;
mod model;
mod report;
mod run;

pub use discover::discover_scenarios;
pub use load::SCENARIO_FILENAME;
pub use matcher_kind::{NormalizeScope, default_normalizer};
pub use model::{
    Cmp, Effect, GitCommit, GitScript, MatcherKind, Requires, Scenario, Step, StreamExpectation,
};
pub use report::{Report, StepOutcome, StepStatus};
pub use run::{CACHE_DIR_ENV, apply_git_script, run_scenario, run_scenario_with};
