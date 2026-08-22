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
use toven_core::federation::member_ecosystem_adapters;
use toven_model::{AbsPath, EcosystemScope};
use toven_ports::{ComputeBudget, Provider};

/// The resolved compute-budget inputs for one APPLY run.
pub(crate) struct BudgetConfig {
    /// Global default budget (flag override, else config default).
    pub(crate) global: ComputeBudget,
    /// Per-scope (member + ecosystem) budget overrides.
    pub(crate) overrides: BTreeMap<EcosystemScope, ComputeBudget>,
    /// Per-scope (member + ecosystem) env-var names each fanned-out tool's
    /// share is injected through.
    pub(crate) env: BTreeMap<EcosystemScope, Vec<String>>,
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
/// Resolution spans the whole federation: each umbrella member carries its own
/// authoritative `[ecosystems.*]` (the umbrella file contributes only
/// cross-member settings), so the inputs are keyed by
/// [`EcosystemScope`] (member + ecosystem). That both reaches member-level
/// config the umbrella document does not hold *and* keeps two members' shared
/// ecosystem (`go`) apart when they size it differently. The degenerate
/// single-repo case is one member under the `None` scope, so it resolves
/// exactly as before.
///
/// # Errors
/// Propagates a member's composition or provider `configure` failure (already
/// surfaced by PLAN, so this re-derive succeeds on the APPLY path).
pub(crate) fn resolve(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    flag_override: Option<ComputeBudget>,
) -> AppResult<BudgetConfig> {
    let global = flag_override.unwrap_or(document.toven.compute_budget);
    let adapters = member_ecosystem_adapters(project_root, document, providers)?;
    let mut overrides = BTreeMap::new();
    let mut env = BTreeMap::new();
    for (member, ecosystem, adapter) in adapters.iter() {
        let scope = EcosystemScope::new(member.cloned(), ecosystem.clone());
        // A CLI `--compute-budget` overrides the whole run, so per-ecosystem
        // config overrides are ignored while it is present — otherwise the flag
        // could not opt out an ecosystem that config had pinned to a budget.
        if flag_override.is_none()
            && let Some(budget) = adapter.common().compute_budget
        {
            overrides.insert(scope.clone(), budget);
        }
        let names = adapter.compute_budget_env();
        if !names.is_empty() {
            env.insert(scope, names);
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
    use toven_model::{AbsPath, EcosystemId, EcosystemScope, MemberId};
    use toven_ports::{CommonEcosystemConfig, ComputeBudget, Provider};
    use toven_testkit::{FakeConfiguredAdapter, FakeProvider, document_path};

    use super::resolve;

    fn go() -> EcosystemId {
        EcosystemId::new("go").expect("valid id")
    }

    fn go_scope() -> EcosystemScope {
        EcosystemScope::bare(go())
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

    /// A throwaway absolute root for the degenerate (single-repo) case. Composition
    /// of a lone `[project]` member never reads the filesystem, so any absolute
    /// path serves.
    fn scratch_root() -> AbsPath {
        AbsPath::new(std::env::temp_dir().join("toven-compute-budget-resolve")).expect("abs path")
    }

    #[test]
    fn a_compute_budget_flag_suppresses_per_ecosystem_overrides() {
        let provider = go_provider(Some(ComputeBudget::fixed(8)));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let document = polyglot_document();
        let root = scratch_root();

        // With the flag, the config override is dropped so the flag wins across
        // every ecosystem — `--compute-budget inherit` truly opts Go out.
        let config = resolve(&root, &document, &providers, Some(ComputeBudget::Inherit))
            .expect("resolves with a flag");
        assert_eq!(config.global, ComputeBudget::Inherit);
        assert!(
            config.overrides.is_empty(),
            "a flag override suppresses per-ecosystem config overrides, got {:?}",
            config.overrides,
        );
        assert_eq!(
            config.env.get(&go_scope()).map(Vec::as_slice),
            Some(["GOMAXPROCS".to_string()].as_slice()),
            "the injected env name is unaffected by the flag",
        );

        // Without a flag, the configured per-ecosystem override applies as set.
        let config = resolve(&root, &document, &providers, None).expect("resolves without a flag");
        assert_eq!(config.global, ComputeBudget::fixed(6));
        assert_eq!(
            config.overrides.get(&go_scope()),
            Some(&ComputeBudget::fixed(8))
        );
    }

    #[test]
    fn resolves_member_level_config_across_a_federation() {
        // A cross-repo umbrella whose root has no `[ecosystems.*]`: the Go
        // config lives only in each member's own `toven.toml`. Resolution must
        // reach every member and key its inputs by member, so member Go units
        // are injected (rather than silently getting nothing) and two members'
        // budgets stay under distinct keys rather than collapsing.
        let ws = toven_testkit::workspace::workspace("compute-budget-federation");
        ws.write_file(
            "repos/core/toven.toml",
            b"[project]\nname = \"core\"\n[ecosystems.go]\nmodules = [\".\"]\n",
        )
        .expect("write core");
        ws.write_file(
            "repos/services/toven.toml",
            b"[project]\nname = \"services\"\n[ecosystems.go]\nmodules = [\".\"]\n",
        )
        .expect("write services");
        ws.write_file(
            "toven.toml",
            b"[project]\nname = \"umbrella\"\n[[members]]\nname = \"core\"\nroot = \"repos/core\"\n[[members]]\nname = \"services\"\nroot = \"repos/services\"\n",
        )
        .expect("write umbrella");

        let root = AbsPath::new(ws.path().to_path_buf()).expect("abs path");
        let loaded = BTreeSet::from([go()]);
        let document = load(
            root.as_path().join("toven.toml"),
            &loaded,
            &CanonicalRegistry::model(),
        )
        .expect("loads umbrella")
        .document;

        // The fake configures every member's Go section into the same adapter
        // (registering `GOMAXPROCS` and a per-ecosystem budget), so a match on
        // each member proves the inputs are keyed by member, not collapsed.
        let provider = go_provider(Some(ComputeBudget::fixed(12)));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let config = resolve(&root, &document, &providers, None).expect("resolves federation");

        let core = EcosystemScope::new(Some(MemberId::new("core").expect("id")), go());
        let services = EcosystemScope::new(Some(MemberId::new("services").expect("id")), go());
        // Both members expose Go, so both register the env knob under their own
        // member scope — no collapse onto a single ecosystem key, and neither
        // member's units run without an injection.
        assert_eq!(
            config.env.get(&core).map(Vec::as_slice),
            Some(["GOMAXPROCS".to_string()].as_slice()),
        );
        assert_eq!(
            config.env.get(&services).map(Vec::as_slice),
            Some(["GOMAXPROCS".to_string()].as_slice()),
        );
        // The umbrella has no root `[ecosystems.go]`, so the `None` scope is absent.
        assert!(!config.env.contains_key(&go_scope()));
        // Each member's `[ecosystems.go].compute_budget` override is captured
        // under its own member key rather than overwriting a shared one.
        assert_eq!(config.overrides.get(&core), Some(&ComputeBudget::fixed(12)));
        assert_eq!(
            config.overrides.get(&services),
            Some(&ComputeBudget::fixed(12))
        );
        assert!(!config.overrides.contains_key(&go_scope()));
    }
}
