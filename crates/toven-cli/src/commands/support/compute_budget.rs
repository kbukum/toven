//! Resolve the compute-budget inputs for [`ApplyOptions`](toven_engine::apply::ApplyOptions).
//!
//! The task-execution verbs ([`run`](super::super::run) and
//! [`watch`](super::super::watch)) size CPU-bound tool fan-out the same way: a
//! global budget (the `--compute-budget` flag, else `[toven].compute_budget`),
//! the per-ecosystem `[ecosystems.<id>].compute_budget` overrides, and the
//! ecosystem→env-name map read from each configured adapter's
//! [`compute_budget_env`](toven_ports::ConfiguredAdapter::compute_budget_env).
//! Assembling it once here keeps the two verbs consistent. The release tail
//! does not consume it — its build/package step is a single self-balancing
//! `cargo` invocation that registers no env name anyway.

use std::collections::BTreeMap;

use rskit_errors::AppResult;
use toven_core::config::Document;
use toven_core::plan::configure::configure;
use toven_model::EcosystemId;
use toven_ports::{ComputeBudget, Provider};

/// The resolved compute-budget inputs for one APPLY run.
pub(crate) struct BudgetConfig {
    /// Global default budget (flag override, else config default).
    pub(crate) global: ComputeBudget,
    /// Per-ecosystem budget overrides.
    pub(crate) overrides: BTreeMap<EcosystemId, ComputeBudget>,
    /// Per-ecosystem env-var names each fanned-out tool's share is injected
    /// through.
    pub(crate) env: BTreeMap<EcosystemId, Vec<String>>,
}

impl BudgetConfig {
    /// Apply these inputs onto an [`ApplyOptions`](toven_engine::apply::ApplyOptions).
    pub(crate) fn apply_to(self, options: &mut toven_engine::apply::ApplyOptions) {
        options.compute_budget = self.global;
        options.ecosystem_budgets = self.overrides;
        options.budget_env = self.env;
    }
}

/// Resolve the compute-budget inputs from the configured adapters and the
/// global budget selection.
///
/// `flag_override` is the `--compute-budget` CLI value when present; it wins
/// over `[toven].compute_budget`. Per-ecosystem overrides and the injected env
/// names both come from each ecosystem's configured adapter, so only ecosystems
/// that declare a fan-out knob (e.g. Go's `GOMAXPROCS`) are ever injected.
///
/// # Errors
/// Propagates a provider's `configure` failure (already surfaced by PLAN, so
/// this re-parse succeeds on the APPLY path).
pub(crate) fn resolve(
    providers: &[&dyn Provider],
    document: &Document,
    flag_override: Option<ComputeBudget>,
) -> AppResult<BudgetConfig> {
    let global = flag_override.unwrap_or(document.toven.compute_budget);
    let configured = configure(document, providers)?;
    let mut overrides = BTreeMap::new();
    let mut env = BTreeMap::new();
    for (ecosystem, adapter) in &configured {
        if let Some(budget) = adapter.common().compute_budget {
            overrides.insert(ecosystem.clone(), budget);
        }
        let names = adapter.compute_budget_env();
        if !names.is_empty() {
            env.insert(ecosystem.clone(), names);
        }
    }
    Ok(BudgetConfig {
        global,
        overrides,
        env,
    })
}
