//! The PLAN pipeline: drive the Configure→Schedule phases and emit PHASE/PLAN events → immutable `Plan`.
//!
//! Load ran in [`config`](crate::config) before this entry. The pipeline
//! configures adapters, discovers the full federation, builds + semantically
//! validates the graph, resolves the active set and per-workspace toolchains, then
//! schedules the federated waves and bakes a static cache verdict into every unit.

use rskit_errors::AppResult;
use toven_model::{Event, ExecutionUnit, ModuleKey, Phase, Plan};
use toven_ports::{Provider, Reporter};

use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;

use super::cache::{self, KeyInputs};
use super::host::PlanHost;
use super::request::PlanRequest;
use super::{affected, discover, front, schedule, toolchain};

/// Run the pure PLAN pipeline, producing one immutable [`Plan`].
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
    let locator = PathDriverLocator::new();
    let context = front::prepare(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Affected,
    })?;
    let active = affected::active_modules(request, &context.graph, &context.federation, host.vcs)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Affected,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Toolchain,
    })?;
    let toolchains = toolchain::resolve(
        &request.project_root,
        &context.federation,
        &active,
        &context.adapters,
        host.prober,
    )?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Toolchain,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Schedule,
    })?;
    let active_list: Vec<ModuleKey> = active.iter().cloned().collect();
    let scheduled = schedule::schedule(
        request,
        &context.federation,
        &active_list,
        &context.adapters,
        &context.group_overrides,
        &toolchains,
    )?;
    let units = decide_cache(
        request,
        &context.federation,
        &context.graph,
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
    let unit_modules: Vec<ModuleKey> = scheduled
        .units
        .iter()
        .flat_map(|planned| planned.members.iter().cloned())
        .collect();
    let needed = cache::needed_modules(&unit_modules, &adjacency);
    let needed_modules: Vec<toven_model::Module> = federation
        .modules
        .iter()
        .filter(|module| needed.contains(&module.key()))
        .cloned()
        .collect();
    let hashes = cache::source_hashes(&needed_modules, host.digest)?;
    let passthrough_present = !request.passthrough.is_empty();

    let mut units = Vec::with_capacity(scheduled.units.len());
    for planned in &scheduled.units {
        // Persistent units never cache; for the rest, `cache::verdict` derives
        // the content key only when the verdict needs it (Force / ReadWrite),
        // skipping wasted digest work and avoidable I/O errors for Disabled
        // units.
        let (verdict, key) = if planned.persistent {
            (toven_model::CacheVerdict::Disabled, None)
        } else {
            cache::verdict(
                request.cache_mode,
                planned.cache_args,
                passthrough_present,
                host.cache,
                || {
                    let inputs = KeyInputs {
                        modules: &planned.members,
                        base_argv: &planned.base_argv,
                        shared_inputs: &planned.shared_inputs,
                        toolchain_identity: &planned.toolchain_identity,
                        cache_args: planned.cache_args,
                        passthrough: &request.passthrough,
                    };
                    cache::unit_key(&inputs, &adjacency, &hashes, host.digest)
                },
            )?
        };
        reporter.emit(&Event::CacheDecided {
            unit_id: planned.id.clone(),
            verdict,
        })?;
        units.push(ExecutionUnit {
            id: planned.id.clone(),
            module: planned.module.clone(),
            members: planned.members.clone(),
            kind: planned.kind.clone(),
            origin: planned.origin,
            workspace: planned.workspace.clone(),
            argv: planned.argv.clone(),
            persistent: planned.persistent,
            readiness: planned.readiness.clone(),
            readiness_timeout: planned.readiness_timeout,
            cache: verdict,
            cache_key: cacheable_key(verdict, key),
            depends_on: planned.depends_on.clone(),
            resource_group: planned.resource_group.clone(),
        });
    }
    Ok(units)
}

/// The key to persist for a unit: a freshly computed key is recorded only for a
/// `Miss` (a new entry to write) or a `Forced` run (which overwrites), and is
/// dropped for `Hit`/`Disabled` outcomes that never write a record.
fn cacheable_key(verdict: toven_model::CacheVerdict, key: Option<String>) -> Option<String> {
    key.filter(|_| {
        matches!(
            verdict,
            toven_model::CacheVerdict::Miss | toven_model::CacheVerdict::Forced
        )
    })
}
