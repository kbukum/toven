//! Shared PLAN front half: Configure → Discover → Graph.
//!
//! Task planning and release planning both need the same validated federation
//! before they diverge into their domain-specific tails. This module keeps that
//! seam engine-internal while avoiding duplicate discovery/graph policy.

use rskit_errors::AppResult;
use toven_model::{AbsPath, Event, Graph, Phase};
use toven_ports::{DriverLocator, Provider, Reporter};

use crate::config::Document;
use crate::federation::compose::ComposedFederation;
use crate::federation::spine;

use super::configure::MemberAdapters;
use super::discover::Federation;
use super::graph;

/// Validated shared state produced before a PLAN tail diverges.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct PlanContext {
    pub(crate) composed: ComposedFederation,
    pub(crate) adapters: MemberAdapters,
    pub(crate) federation: Federation,
    pub(crate) graph: Graph,
}

/// Run the reusable Configure → Discover → Graph front half.
///
/// `project_root` is the umbrella project root; members (if any) resolve under
/// it. Configure bakes every member's adapters, Discover unions every member's
/// rebased discovery output into one federation, and Graph builds + semantically
/// validates the result.
///
/// # Errors
/// Propagates configuration, discovery, graph construction, or semantic
/// validation failures.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepare(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
    reporter: &mut dyn Reporter,
) -> AppResult<PlanContext> {
    let composed = spine::compose(project_root, document, providers)?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Configure,
    })?;
    let (adapters, configure_warnings) = spine::configure_all(&composed, providers, locator)?;
    // Surface absent-driver skips while still in Configure, before discovery can
    // fail: a warn-and-skip is observable even on a later failure, never silently
    // dropped, and keeps the phase attribution accurate.
    for warning in &configure_warnings {
        reporter.emit(&Event::Warning {
            message: warning.clone(),
        })?;
    }
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Configure,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Discover,
    })?;
    let federation = spine::discover_all(project_root, &composed, &adapters)?;
    // Surface adapter discovery warnings so a warn-and-skip is observable here too.
    for warning in &federation.warnings {
        reporter.emit(&Event::Warning {
            message: warning.clone(),
        })?;
    }
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Discover,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Graph,
    })?;
    let graph = graph::build(&federation)?;
    graph::validate_semantics(&graph, &composed)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Graph,
    })?;

    Ok(PlanContext {
        composed,
        adapters,
        federation,
        graph,
    })
}

/// Build the validated discovered module dependency graph without scheduling a task.
///
/// This runs the shared Configure → Discover → Graph front half and stops before
/// affected selection, toolchain probing, scheduling, or cache decisions. Use it
/// for introspection surfaces whose output is about the discovered topology rather
/// than a task-specific PLAN cut. The injected `locator` resolves out-of-process
/// drivers for canonical-but-unloaded ecosystems, so federation resolution stays
/// deterministic in tests.
///
/// # Errors
/// Propagates configuration, discovery, graph construction, or semantic
/// validation failures.
pub fn dependency_graph(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
    reporter: &mut dyn Reporter,
) -> AppResult<Graph> {
    prepare(project_root, document, providers, locator, reporter).map(|context| context.graph)
}
