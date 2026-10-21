//! The PLAN pipeline: drive the Configure→Schedule phases and emit PHASE/PLAN
//! events → immutable `Plan`.
//!
//! Load ran in [`config`](crate::config) before this entry. The pipeline
//! configures adapters, discovers the full federation, builds + semantically
//! validates the graph, resolves the active set and per-workspace toolchains,
//! then schedules the federated waves and bakes a static cache verdict into
//! every unit.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{EcosystemId, Event, ExecutionUnit, ModuleKey, Phase, Plan};
use toven_ports::{Provider, Reporter};

use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;

use super::cache::{self, KeyInputs};
use super::configure::MemberAdapters;
use super::host::PlanHost;
use super::request::{PlanRequest, Selection};
use super::{affected, discover, front, schedule, toolchain};

type CacheWorkspace = (
    Option<toven_model::MemberId>,
    EcosystemId,
    Option<toven_model::WorkspaceId>,
);
type WorkspaceModules = BTreeMap<CacheWorkspace, Vec<ModuleKey>>;

/// A plan projected for `explain`, paired with the resolved display focus.
///
/// [`plan_focused`] builds `plan` over the request's scope selection and, when
/// a `focus` selection is given, resolves it to the concrete module keys the
/// CLI narrows the shown units to. `focus` is `None` for an unfocused
/// projection.
#[derive(Debug)]
pub struct FocusedPlan {
    /// The immutable plan over the request's scope selection.
    pub plan: Plan,
    /// The module keys an explicit display focus resolved to, when given.
    pub focus: Option<BTreeSet<ModuleKey>>,
}

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
    plan_focused(request, None, document, providers, host, reporter).map(|focused| focused.plan)
}

/// Run the PLAN pipeline and, when `focus` is given, additionally resolve it to
/// the module keys an `explain --module`/`--workspace` projection filters to.
///
/// The `plan` is always built over `request.selection` (the scope); the `focus`
/// selection never changes what is planned or how units batch — it only
/// resolves, against the same discovered graph, which modules the caller wants
/// to inspect. This keeps `explain --module X` showing the *real* batched unit
/// `X` belongs to rather than a synthetic single-module cut. Focus resolution
/// reuses each task's recognized intent, so its dependency/dependents closures
/// honor the same kind-aware edge rules; a task list's focus is their union.
///
/// # Errors
/// Propagates any phase failure (configure/discover/graph/affected/toolchain/
/// schedule/cache) and any focus selector-resolution failure (unknown or
/// ambiguous target).
pub fn plan_focused(
    request: &PlanRequest,
    focus: Option<&Selection>,
    document: &Document,
    providers: &[&dyn Provider],
    host: PlanHost<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<FocusedPlan> {
    let locator = PathDriverLocator::new();
    let context = front::prepare(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;

    // Every requested task is planned against the one discovered federation and
    // merged into a single plan: units are task-qualified (`module#task`), so the
    // tasks never collide and run side by side under one scheduler. The phases
    // run task-by-task inside each phase so every phase is reported once.
    let requests = request
        .split_by_intent()
        .into_iter()
        .map(|single| {
            // Recognition is config-authoritative: the addressed task's `kind`
            // attribute supersedes the name-derived default so a renamed task
            // (`my-test` with `kind = "test"`) drives the kind-aware dev-edge rule.
            Ok(recognize_intent(&single, &context.adapters)?.unwrap_or(single))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let (units, waves) = plan_intents(&requests, &context, host, reporter)?;
    ensure_unique_unit_ids(&units)?;

    reporter.emit(&Event::PlanPrepared {
        waves: waves.len(),
        units: units.len(),
    })?;

    // Each task's closure follows its own kind-aware edge rules; a list's
    // focus is their union.
    let focus = focus
        .map(|selection| {
            requests
                .iter()
                .try_fold(BTreeSet::new(), |mut keys, single| {
                    keys.extend(resolve_focus(single, selection, &context, host)?);
                    Ok::<_, AppError>(keys)
                })
        })
        .transpose()?;

    Ok(FocusedPlan {
        plan: Plan::new(units, waves),
        focus,
    })
}

/// Plan every recognized single-task request (Affected→Schedule) against the
/// prepared federation, merging units and same-index waves into one plan.
fn plan_intents(
    requests: &[PlanRequest],
    context: &front::PlanContext,
    host: PlanHost<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<(Vec<ExecutionUnit>, Vec<Vec<String>>)> {
    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Affected,
    })?;
    let explicit = if requests.len() > 1 {
        affected::ExplicitScope::Narrow
    } else {
        affected::ExplicitScope::Strict
    };
    let mut actives = Vec::with_capacity(requests.len());
    for request in requests {
        let active =
            affected::active_modules(request, &context.graph, &context.federation, host.vcs)?;
        let active = affected::restrict_to_task_defining(
            active,
            request,
            &context.federation,
            &context.adapters,
            explicit,
        )?;
        if !active.full_activation.is_empty() {
            reporter.emit(&Event::FullActivation {
                paths: active.full_activation.clone(),
            })?;
        }
        actives.push(active);
    }
    ensure_some_task_applies(requests, &actives, explicit)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Affected,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Toolchain,
    })?;
    let mut toolchains = Vec::with_capacity(requests.len());
    for (request, active) in requests.iter().zip(&actives) {
        toolchains.push(toolchain::resolve(
            &request.project_root,
            &context.federation,
            &active.modules,
            &context.adapters,
            host.prober,
            &request.intent,
        )?);
    }
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Toolchain,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Schedule,
    })?;
    let mut units = Vec::new();
    let mut waves: Vec<Vec<String>> = Vec::new();
    for ((request, active), toolchains) in requests.iter().zip(&actives).zip(&toolchains) {
        let active_list: Vec<ModuleKey> = active.modules.iter().cloned().collect();
        let scheduled = schedule::schedule(
            request,
            &context.federation,
            &active_list,
            &context.adapters,
            &context.group_overrides,
            toolchains,
        )?;
        units.extend(decide_cache(
            request,
            &context.federation,
            &context.graph,
            &scheduled,
            host,
            reporter,
        )?);
        for (index, wave) in scheduled.waves.into_iter().enumerate() {
            match waves.get_mut(index) {
                Some(merged) => merged.extend(wave),
                None => waves.push(wave),
            }
        }
    }
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Schedule,
    })?;
    Ok((units, waves))
}

/// Reject a task list whose tasks all skip every explicitly selected module, so
/// a narrowed list never turns a mistyped selection into an empty success.
fn ensure_some_task_applies(
    requests: &[PlanRequest],
    actives: &[affected::ActiveModules],
    explicit: affected::ExplicitScope,
) -> AppResult<()> {
    let Some(first) = requests.first() else {
        return Ok(());
    };
    if explicit != affected::ExplicitScope::Narrow
        || !matches!(first.selection, Selection::Explicit { .. })
        || actives.iter().any(|active| !active.modules.is_empty())
    {
        return Ok(());
    }
    let tasks: Vec<&str> = requests
        .iter()
        .map(|request| request.intent.name())
        .collect();
    Err(AppError::new(
        rskit_errors::ErrorCode::InvalidInput,
        format!(
            "none of the tasks {} applies to the selected modules",
            tasks.join(", ")
        ),
    ))
}

/// Reject a merged plan in which two tasks produced the same unit id.
fn ensure_unique_unit_ids(units: &[ExecutionUnit]) -> AppResult<()> {
    let mut seen = BTreeSet::new();
    for unit in units {
        if !seen.insert(unit.id.as_str()) {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!("two planned tasks produced the same unit '{}'", unit.id),
            ));
        }
    }
    Ok(())
}

/// Resolve an `explain` display focus to concrete module keys.
///
/// Reuses the same [`affected::active_modules`] resolution the scope uses, so
/// an explicit focus selector (and its optional dependency/dependents closures)
/// maps to modules exactly as a real selection would, including the typed
/// unknown/ ambiguous-target errors.
fn resolve_focus(
    request: &PlanRequest,
    selection: &Selection,
    context: &front::PlanContext,
    host: PlanHost<'_>,
) -> AppResult<BTreeSet<ModuleKey>> {
    let mut focus_request = request.clone();
    focus_request.selection = selection.clone();
    affected::active_modules(
        &focus_request,
        &context.graph,
        &context.federation,
        host.vcs,
    )
    .and_then(|active| {
        affected::restrict_to_task_defining(
            active,
            &focus_request,
            &context.federation,
            &context.adapters,
            affected::ExplicitScope::Strict,
        )
        .map(|active| active.modules)
    })
}

/// Resolve the run's recognized kind from the configured task tables.
///
/// Returns a request with the addressed task's configured `kind` when it
/// differs from the name-derived default (so `my-test` with `kind = "test"` is
/// recognized as a Test run), or `None` when the token-derived kind already
/// stands.
///
/// The recognized kind is the single
/// non-[`Default`](toven_ports::TaskKind::Default) kind configured for the
/// addressed name across every ecosystem. It is order-independent: all
/// declaring ecosystems must agree. A cross-ecosystem conflict (one tags the
/// name `test`, another `build`) is rejected with an actionable error rather
/// than resolved by arbitrary iteration order.
///
/// # Errors
/// Returns [`AppError::invalid_input`] when two ecosystems configure the same
/// task name with different recognized kinds.
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
    let workspace_modules = workspace_cache_modules(federation);
    let scopes = unit_cache_scopes(scheduled, &workspace_modules)?;
    let unit_modules: Vec<ModuleKey> = scopes
        .iter()
        .flat_map(|members| members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let needed = cache::needed_modules(&unit_modules, &adjacency);
    let needed_modules: Vec<toven_model::Module> = federation
        .modules
        .iter()
        .filter(|module| needed.contains(&module.key()))
        .cloned()
        .collect();
    let hashes = cache::source_hashes(&needed_modules, host.digest)?;
    let incomplete: BTreeSet<_> = needed_modules
        .iter()
        .filter(|module| !module.cache_inputs_complete)
        .map(toven_model::Module::key)
        .collect();
    let passthrough_present = !request.passthrough.is_empty();

    let mut units = Vec::with_capacity(scheduled.units.len());
    for (planned, scope) in scheduled.units.iter().zip(scopes) {
        // Persistent units never cache; neither do units whose task opted out
        // (`cacheable = false`) — a mutating `*-fix` task, where a stale content-key
        // hit would wrongly suppress the mutation. Incomplete mutable source
        // closures also disable caching, regardless of task overrides or Force.
        // For the rest, `cache::verdict`
        // derives the content key only when the verdict needs it (Force / ReadWrite),
        // skipping wasted digest work and avoidable I/O errors for Disabled units.
        let complete = incomplete.is_empty()
            || cache::needed_modules(scope, &adjacency).is_disjoint(&incomplete);
        let (verdict, key) = if planned.persistent || !planned.cacheable || !complete {
            (toven_model::CacheVerdict::Disabled, None)
        } else {
            cache::verdict(
                request.cache_mode,
                planned.cache_args,
                passthrough_present,
                host.cache,
                || {
                    let inputs = KeyInputs {
                        modules: scope,
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
            fail_if_output: planned.fail_if_output,
            cache: verdict,
            cache_key: cacheable_key(verdict, key),
            depends_on: planned.depends_on.clone(),
            resource_group: planned.resource_group.clone(),
        });
    }
    Ok(units)
}

fn workspace_cache_modules(federation: &discover::Federation) -> WorkspaceModules {
    let mut workspaces = WorkspaceModules::new();
    for module in &federation.modules {
        let key = (
            module.member.clone(),
            module.id.ecosystem.clone(),
            module.workspace.clone(),
        );
        workspaces.entry(key).or_default().push(module.key());
    }
    workspaces
}

fn unit_cache_scopes<'a>(
    scheduled: &'a schedule::Scheduled,
    workspaces: &'a WorkspaceModules,
) -> AppResult<Vec<&'a [ModuleKey]>> {
    scheduled
        .units
        .iter()
        .map(|unit| {
            if !unit.whole_workspace {
                return Ok(unit.members.as_slice());
            }
            let key = (
                unit.module.member.clone(),
                unit.module.module.ecosystem.clone(),
                unit.workspace.clone(),
            );
            workspaces.get(&key).map(Vec::as_slice).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("unit '{}' has no workspace source scope", unit.id),
                )
            })
        })
        .collect()
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
