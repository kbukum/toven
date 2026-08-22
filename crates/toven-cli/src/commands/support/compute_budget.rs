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
/// `flag_override` is the `--compute-budget` CLI value when present. It is a
/// whole-run override: it wins over `[toven].compute_budget` *and* over every
/// per-ecosystem `[ecosystems.<id>].compute_budget`, so the documented CLI
/// escape hatch (`--compute-budget inherit`) actually opts every ecosystem out
/// rather than being silently overridden by a config override that the engine's
/// `BudgetPlan` would give precedence. When no flag is present the per-ecosystem
/// overrides apply as configured. The injected env names always come from each
/// ecosystem's configured adapter, so only ecosystems that declare a fan-out
/// knob (e.g. Go's `GOMAXPROCS`) are ever injected.
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
        // A CLI `--compute-budget` overrides the whole run, so per-ecosystem
        // config overrides are ignored while it is present — otherwise the flag
        // could not opt out an ecosystem that config had pinned to a budget.
        if flag_override.is_none()
            && let Some(budget) = adapter.common().compute_budget
        {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use toven_core::config::{CanonicalRegistry, Document, load};
    use toven_model::EcosystemId;
    use toven_ports::{CommonEcosystemConfig, ComputeBudget, Provider};
    use toven_testkit::{FakeConfiguredAdapter, FakeProvider, document_path};

    use super::resolve;

    fn go() -> EcosystemId {
        EcosystemId::new("go").expect("valid id")
    }

    /// A Go provider whose configured adapter registers `GOMAXPROCS` and carries
    /// the given per-ecosystem `[ecosystems.go].compute_budget` override.
    fn go_provider(override_budget: Option<ComputeBudget>) -> FakeProvider {
        let common = CommonEcosystemConfig {
            compute_budget: override_budget,
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(go())
            .with_compute_budget_env(vec!["GOMAXPROCS".to_string()])
            .with_common(common);
        FakeProvider::new(go()).with_adapter(adapter)
    }

    /// The shared polyglot fixture (`[toven].compute_budget = 6`, `rust` + `go`).
    fn polyglot_document() -> Document {
        let path = document_path("valid/polyglot.toml").expect("fixture path");
        let loaded: BTreeSet<EcosystemId> = ["rust", "go"]
            .iter()
            .map(|id| EcosystemId::new(*id).expect("valid id"))
            .collect();
        load(&path, &loaded, &CanonicalRegistry::model())
            .expect("loads")
            .document
    }

    #[test]
    fn a_compute_budget_flag_suppresses_per_ecosystem_overrides() {
        let provider = go_provider(Some(ComputeBudget::fixed(8)));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let document = polyglot_document();

        // With the flag, the config override is dropped so the flag wins across
        // every ecosystem — `--compute-budget inherit` truly opts Go out.
        let config = resolve(&providers, &document, Some(ComputeBudget::Inherit))
            .expect("resolves with a flag");
        assert_eq!(config.global, ComputeBudget::Inherit);
        assert!(
            config.overrides.is_empty(),
            "a flag override suppresses per-ecosystem config overrides, got {:?}",
            config.overrides,
        );
        assert_eq!(
            config.env.get(&go()).map(Vec::as_slice),
            Some(["GOMAXPROCS".to_string()].as_slice()),
            "the injected env name is unaffected by the flag",
        );

        // Without a flag, the configured per-ecosystem override applies as set.
        let config = resolve(&providers, &document, None).expect("resolves without a flag");
        assert_eq!(config.global, ComputeBudget::fixed(6));
        assert_eq!(config.overrides.get(&go()), Some(&ComputeBudget::fixed(8)));
    }
}
