//! Shared PLAN front half: Configure → Discover → Graph.
//!
//! Task planning and release planning both need the same validated federation
//! before they diverge into their domain-specific tails. This module keeps that
//! seam engine-internal while avoiding duplicate discovery/graph policy.

use rskit_errors::AppResult;
use toven_model::{AbsPath, Event, Graph, Phase};
use toven_ports::{Provider, Reporter};

use crate::config::Document;

use super::configure::ConfiguredSet;
use super::discover::Federation;
use super::{configure, discover, graph};

/// Validated shared state produced before a PLAN tail diverges.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct PlanContext {
    pub(crate) adapters: ConfiguredSet,
    pub(crate) federation: Federation,
    pub(crate) graph: Graph,
}

/// Run the reusable Configure → Discover → Graph front half.
///
/// # Errors
/// Propagates configuration, discovery, graph construction, or semantic
/// validation failures.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepare(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    reporter: &mut dyn Reporter,
) -> AppResult<PlanContext> {
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
    let federation = discover::discover(project_root, &adapters, document)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Discover,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Graph,
    })?;
    let graph = graph::build(&federation)?;
    graph::validate_semantics(&graph, document)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Graph,
    })?;

    Ok(PlanContext {
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
/// than a task-specific PLAN cut.
///
/// # Errors
/// Propagates configuration, discovery, graph construction, or semantic
/// validation failures.
pub fn dependency_graph(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    reporter: &mut dyn Reporter,
) -> AppResult<Graph> {
    prepare(project_root, document, providers, reporter).map(|context| context.graph)
}
