//! The PLAN pipeline: drive the Configure→Schedule phases and emit PHASE/PLAN events → immutable `Plan`.
//!
//! Load ran in [`config`](crate::config) before this entry. The pipeline
//! configures adapters, discovers the full federation, builds + semantically
//! validates the graph, resolves the active set and per-workspace toolchains, then
//! schedules the federated waves and bakes a static cache verdict into every unit.

use rskit_errors::{AppError, AppResult};
use toven_model::{EcosystemId, Event, ExecutionUnit, ModuleKey, Phase, Plan};
use toven_ports::{Provider, Reporter};

use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;

use super::cache::{self, KeyInputs};
use super::configure::MemberAdapters;
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

    // Recognition is config-authoritative: the addressed task's `kind` attribute
    // supersedes the name-derived default so a renamed task (`my-test` with
    // `kind = "test"`) drives the kind-aware dev-edge rule below.
    let recognized = recognize_intent(request, &context.adapters)?;
    let request = recognized.as_ref().unwrap_or(request);

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

/// Resolve the run's recognized kind from the configured task tables.
///
/// Returns a request with the addressed task's configured `kind` when it differs
/// from the name-derived default (so `my-test` with `kind = "test"` is recognized
/// as a Test run), or `None` when the token-derived kind already stands.
///
/// The recognized kind is the single non-[`Default`](toven_ports::TaskKind::Default)
/// kind configured for the addressed name across every ecosystem. It is
/// order-independent: all declaring ecosystems must agree. A cross-ecosystem
/// conflict (one tags the name `test`, another `build`) is rejected with an
/// actionable error rather than resolved by arbitrary iteration order.
///
/// # Errors
/// Returns [`AppError::invalid_input`] when two ecosystems configure the same task
/// name with different recognized kinds.
fn recognize_intent(
    request: &PlanRequest,
    adapters: &MemberAdapters,
) -> AppResult<Option<PlanRequest>> {
    use toven_ports::TaskKind;

    let name = request.intent.name();
    let mut recognized: Option<(TaskKind, &EcosystemId)> = None;
    for (_, ecosystem, adapter) in adapters.iter() {
        let Some(kind) = adapter
            .common()
            .tasks
            .get(name)
            .map(|entry| entry.resolved_kind(name))
            .filter(|kind| *kind != TaskKind::Default)
        else {
            continue;
        };
        match recognized {
            Some((existing, first)) if existing != kind => {
                return Err(AppError::invalid_input(
                    format!("tasks.{name}.kind"),
                    format!(
                        "task '{name}' is configured with conflicting kinds across ecosystems \
                         ('{first}' tags it '{}', '{ecosystem}' tags it '{}'); give the task a \
                         single consistent kind",
                        existing.as_str(),
                        kind.as_str(),
                    ),
                ));
            }
            Some(_) => {}
            None => recognized = Some((kind, ecosystem)),
        }
    }

    let Some((recognized, _)) = recognized else {
        return Ok(None);
    };
    Ok((recognized != request.intent.kind()).then(|| {
        let mut request = request.clone();
        request.intent = request.intent.clone().with_kind(recognized);
        request
    }))
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
            task: planned.task.clone(),
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
