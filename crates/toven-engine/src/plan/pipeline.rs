//! The PLAN pipeline: drive phases 2–7 and emit PHASE/PLAN events → immutable `Plan`.
//!
//! Load (phase 1) ran in [`config`](crate::config) before this entry. The pipeline
//! configures adapters, discovers the full federation, builds + semantically
//! validates the graph, resolves the active set and per-workspace toolchains, then
//! schedules the federated waves and bakes a static cache verdict into every unit.

use rskit_errors::AppResult;
use toven_model::{Event, ExecutionUnit, ModuleRef, Phase, Plan};
use toven_ports::{Provider, Reporter};

use crate::config::Document;

use super::cache::{self, KeyInputs};
use super::host::PlanHost;
use super::request::PlanRequest;
use super::{affected, configure, discover, graph, schedule, toolchain};

/// Run the pure PLAN pipeline, producing one immutable [`Plan`](toven_model::Plan).
///
/// `providers` are the ecosystem adapters compiled into this binary; `host`
/// bundles the injected git/digest/probe/cache effects; `reporter` receives the
/// PHASE and PLAN events.
///
/// # Errors
/// Propagates any phase failure (configure/discover/graph/affected/toolchain/
/// schedule/cache).
pub fn plan(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    host: PlanHost<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<Plan> {
    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Configure,
    })?;
    let adapters = configure::configure(document, providers)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Configure,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Discover,
    })?;
    let federation = discover::discover(&request.project_root, &adapters, document)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Discover,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Graph,
    })?;
    let federated_graph = graph::build(&federation)?;
    graph::validate_semantics(&federated_graph, document)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Graph,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Affected,
    })?;
    let active = affected::active_modules(request, &federated_graph, &federation, host.vcs)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Affected,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Toolchain,
    })?;
    let toolchains = toolchain::resolve(
        &request.project_root,
        &federation,
        &active,
        &adapters,
        host.prober,
    )?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Toolchain,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Schedule,
    })?;
    let active_list: Vec<ModuleRef> = active.iter().cloned().collect();
    let scheduled = schedule::schedule(request, &federation, &active_list, &adapters, &toolchains)?;
    let units = decide_cache(
        request,
        &federation,
        &federated_graph,
        &scheduled,
        host,
        reporter,
    )?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Schedule,
    })?;

    reporter.emit(&Event::PlanPrepared {
        waves: scheduled.waves.len(),
        units: units.len(),
    })?;

    Ok(Plan::new(units, scheduled.waves))
}

/// Compute each unit's static cache verdict and emit `CacheDecided`.
fn decide_cache(
    request: &PlanRequest,
    federation: &discover::Federation,
    graph: &toven_model::Graph,
    scheduled: &schedule::Scheduled,
    host: PlanHost<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<Vec<ExecutionUnit>> {
    let adjacency = cache::forward_adjacency(graph);
    let unit_modules: Vec<ModuleRef> = scheduled
        .units
        .iter()
        .map(|planned| planned.module.clone())
        .collect();
    let needed = cache::needed_modules(&unit_modules, &adjacency);
    let needed_modules: Vec<toven_model::Module> = federation
        .modules
        .iter()
        .filter(|module| needed.contains(&module.id))
        .cloned()
        .collect();
    let hashes = cache::source_hashes(&needed_modules, host.digest)?;
    let passthrough_present = !request.passthrough.is_empty();

    let mut units = Vec::with_capacity(scheduled.units.len());
    for planned in &scheduled.units {
        let inputs = KeyInputs {
            module: &planned.module,
            base_argv: &planned.base_argv,
            shared_inputs: &planned.shared_inputs,
            toolchain_identity: &planned.toolchain_identity,
            cache_args: planned.cache_args,
            passthrough: &request.passthrough,
        };
        let key = cache::unit_key(&inputs, &adjacency, &hashes, host.digest)?;
        let verdict = if planned.persistent {
            toven_model::CacheVerdict::Disabled
        } else {
            cache::verdict(
                request.cache_mode,
                planned.cache_args,
                passthrough_present,
                &key,
                host.cache,
            )?
        };
        reporter.emit(&Event::CacheDecided {
            unit_id: planned.id.clone(),
            verdict,
        })?;
        units.push(ExecutionUnit {
            id: planned.id.clone(),
            module: planned.module.clone(),
            kind: planned.kind.clone(),
            workspace: planned.workspace.clone(),
            argv: planned.argv.clone(),
            persistent: planned.persistent,
            readiness: planned.readiness.clone(),
            readiness_timeout: planned.readiness_timeout,
            cache: verdict,
            cache_key: cacheable_key(verdict, &key),
            depends_on: planned.depends_on.clone(),
            resource_group: planned.resource_group.clone(),
        });
    }
    Ok(units)
}

fn cacheable_key(verdict: toven_model::CacheVerdict, key: &str) -> Option<String> {
    matches!(
        verdict,
        toven_model::CacheVerdict::Miss | toven_model::CacheVerdict::Forced
    )
    .then(|| key.to_string())
}
